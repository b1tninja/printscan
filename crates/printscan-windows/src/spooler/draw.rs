//! Drawing a job's pages: each PDF page rendered by the in-box `Windows.Data.Pdf` into a Direct2D command list with
//! its own transform ([`super::placement`]), and the command list added to the job as one page of the paper's size.
//! winprint holds the job (`DxgiPrintContext`); this module draws into it.
//!
//! The calls into Direct2D and `Windows.Data.Pdf`'s native renderer are `unsafe`: COM methods taking interface
//! pointers the compiler cannot check. Each block says what it relies on (docs/wia-unsafe-audit.md, "The spooler's
//! page drawing").

#![allow(unsafe_code)]

use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::path::Path;

use windows::Data::Pdf::PdfDocument;
use windows::Storage::StorageFile;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F;
use windows::Win32::Graphics::Direct2D::{ID2D1CommandList, ID2D1DeviceContext};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::WinRT::Pdf::{IPdfRendererNative, PDF_RENDER_PARAMS, PdfCreateRenderer};
use windows::core::HSTRING;
use windows_numerics::Matrix3x2;
use winprint::printer::{DxgiPrintContext, PrintOutcome};

use super::placement::{Placement, Size, after_scale};

/// COM (and with it WinRT) on this thread for as long as it lives. Created before any other object of the job,
/// so every COM object is released before the apartment is left.
pub(crate) struct Apartment {
    joined: bool,
    /// Not `Send`: COM is left on the thread that entered it.
    _thread: PhantomData<*const ()>,
}

impl Apartment {
    pub fn enter() -> Self {
        // SAFETY: CoInitializeEx takes no pointers but the reserved one, passed as None. A thread already in an
        // apartment of the other kind answers RPC_E_CHANGED_MODE and is left as it was (and not uninitialized on
        // drop); in its own apartment the calls below still work.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        Self { joined, _thread: PhantomData }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.joined {
            // SAFETY: balances the successful CoInitializeEx in `enter`, on the same thread: the type is not Send.
            unsafe { CoUninitialize() };
        }
    }
}

/// A PDF opened by `Windows.Data.Pdf`.
pub(crate) struct Pdf {
    document: PdfDocument,
}

impl Pdf {
    /// Open the file at `path` (absolute: WinRT resolves no relative paths).
    pub fn open(path: &Path) -> windows::core::Result<Self> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.join()?;
        Ok(Self { document: PdfDocument::LoadFromFileAsync(&file)?.join()? })
    }

    /// Each page's size in DIPs, its crop box turned by its `/Rotate`.
    pub fn page_sizes(&self) -> windows::core::Result<Vec<Size>> {
        (0..self.document.PageCount()?)
            .map(|i| {
                let size = self.document.GetPage(i)?.Size()?;
                Ok(Size::new(f64::from(size.Width), f64::from(size.Height)))
            })
            .collect()
    }
}

/// Draws pages into a job: the renderer bound to the job's Direct2D device.
pub(crate) struct Drawer {
    renderer: IPdfRendererNative,
}

impl Drawer {
    pub fn new(device: &IDXGIDevice) -> windows::core::Result<Self> {
        // SAFETY: `device` is the live DXGI device the job draws with (owned by the DxgiPrintContext, which outlives
        // this Drawer in `submit`); PdfCreateRenderer only takes a reference to it.
        Ok(Self { renderer: unsafe { PdfCreateRenderer(device) }? })
    }

    /// Add page `index` of `pdf` (`page` DIPs) to the job, placed on `paper` by `placement`.
    pub fn page(
        &self,
        context: &DxgiPrintContext,
        pdf: &Pdf,
        (index, page): (usize, Size),
        placement: &Placement,
        paper: Size,
    ) -> windows::core::Result<ControlFlow<PrintOutcome>> {
        let pdf_page = pdf.document.GetPage(u32::try_from(index).unwrap_or(u32::MAX))?;
        // Rendered at the job's resolution and scale, so images keep the detail the paper can show and no more.
        let pixels = |dips: f64, dpi: u32| (dips * placement.scale * f64::from(dpi) / 96.0).ceil().clamp(1.0, f64::from(u32::MAX)) as u32;
        let (width, height) = (pixels(page.w, context.dpi.0), pixels(page.h, context.dpi.1));
        let params = PDF_RENDER_PARAMS { DestinationWidth: width, DestinationHeight: height, IgnoreHighContrast: true, ..Default::default() };
        let transform = matrix(after_scale(page.w / f64::from(width), page.h / f64::from(height), placement.matrix));
        let list = record(&context.d2d_context, |dc| {
            // SAFETY: `dc` is the job's device context, between BeginDraw and EndDraw (see `record`); the transform
            // and the render parameters are locals that outlive the calls; `pdf_page` is a live page of the
            // document the renderer reads.
            unsafe {
                dc.SetTransform(&transform);
                let rendered = self.renderer.RenderPageToDeviceContext(&pdf_page, dc, Some(&params));
                dc.SetTransform(&Matrix3x2::identity());
                rendered
            }
        })?;
        context.add_page(&list, size(paper))
    }

    /// Add a blank page (the back of a copy's last sheet) to the job.
    pub fn blank(&self, context: &DxgiPrintContext, paper: Size) -> windows::core::Result<ControlFlow<PrintOutcome>> {
        let list = record(&context.d2d_context, |_| Ok(()))?;
        context.add_page(&list, size(paper))
    }
}

/// A page of the job: the paper, in DIPs.
fn size(paper: Size) -> D2D_SIZE_F {
    D2D_SIZE_F { width: paper.w as f32, height: paper.h as f32 }
}

/// What `draw` draws, recorded into a new command list.
fn record(dc: &ID2D1DeviceContext, draw: impl FnOnce(&ID2D1DeviceContext) -> windows::core::Result<()>) -> windows::core::Result<ID2D1CommandList> {
    // SAFETY: `dc` is the job's live device context; CreateCommandList returns a new list it owns.
    let list = unsafe { dc.CreateCommandList() }?;
    // SAFETY: `list` was just created on `dc`; BeginDraw opens the drawing EndDraw closes below on every path.
    unsafe {
        dc.SetTarget(&list);
        dc.BeginDraw();
    }
    let drawn = draw(dc);
    // SAFETY: closes the BeginDraw above, whether or not drawing succeeded; the out-parameters are not asked for.
    let ended = unsafe { dc.EndDraw(None, None) };
    drawn?;
    ended?;
    // SAFETY: the list is no longer the target of an open drawing; Close makes it ready for the print control.
    unsafe { list.Close() }?;
    Ok(list)
}

fn matrix(m: [f64; 6]) -> Matrix3x2 {
    Matrix3x2 { M11: m[0] as f32, M12: m[1] as f32, M21: m[2] as f32, M22: m[3] as f32, M31: m[4] as f32, M32: m[5] as f32 }
}

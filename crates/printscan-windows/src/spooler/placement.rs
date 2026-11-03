//! Where each page of a job goes on the paper. Windows prints a job with one PrintTicket, so on one paper in one
//! orientation; a PDF's pages need not agree. Each page is drawn with its own transform: a page whose orientation
//! is not the paper's is turned a quarter turn clockwise (as PDF's `/Rotate 90` turns a page), and for
//! [`Fit::Shrink`] a page larger than the printable area is scaled down to fit it, never up. Every page is centred.
//!
//! Sizes are in device-independent pixels (DIPs, 1/96 in), the unit Direct2D draws in and `Windows.Data.Pdf`
//! measures pages in.

/// A width and a height, in DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }

    /// Wider than tall, beyond [`SAME`]: a square page is portrait.
    pub fn landscape(self) -> bool {
        self.w > self.h + SAME
    }

    /// The same size turned a quarter turn.
    pub fn turned(self) -> Self {
        Self::new(self.h, self.w)
    }

    /// Within [`SAME`] of `other` on each side.
    pub fn same_as(self, other: Self) -> bool {
        (self.w - other.w).abs() <= SAME && (self.h - other.h).abs() <= SAME
    }

    /// From micrometres.
    pub fn from_micron(w: u32, h: u32) -> Self {
        let dip = |micron: u32| f64::from(micron) * 96.0 / 25_400.0;
        Self::new(dip(w), dip(h))
    }
}

/// Sizes this close, in DIPs (half a point, 0.18 mm), are the same: PDF writers round metric sizes.
pub(crate) const SAME: f64 = 96.0 / 72.0 * 0.5;

/// How pages meet the paper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fit {
    /// Every page is the paper's size (either way round) and prints at it.
    Exact,
    /// A page larger than the paper less `margin` on every side shrinks to fit there; smaller pages keep their
    /// size. `margin` is the driver's widest unprintable margin, in DIPs.
    Shrink { margin: f64 },
}

/// How one page is drawn on the paper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    /// Turned a quarter turn clockwise.
    pub turned: bool,
    /// The scale, 1 or less.
    pub scale: f64,
    /// The transform from the page's DIPs to the paper's, as Direct2D's `Matrix3x2` holds it:
    /// `[M11, M12, M21, M22, M31, M32]`, so `x' = x·M11 + y·M21 + M31` and `y' = x·M12 + y·M22 + M32`.
    pub matrix: [f64; 6],
}

/// A page that is not the paper's size, for [`Fit::Exact`]: its index and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mismatch {
    pub index: usize,
    pub page: Size,
}

/// Where `page` goes on `paper` (the paper as the ticket orients it).
pub(crate) fn place(page: Size, paper: Size, fit: Fit) -> Option<Placement> {
    let square = (page.w - page.h).abs() <= SAME;
    let turned = !square && page.landscape() != paper.landscape();
    let drawn = if turned { page.turned() } else { page };
    let scale = match fit {
        Fit::Exact => {
            if !drawn.same_as(paper) {
                return None;
            }
            1.0
        }
        Fit::Shrink { margin } => {
            let margin = if margin.is_finite() { margin.max(0.0) } else { 0.0 };
            let room = Size::new((paper.w - 2.0 * margin).max(1.0), (paper.h - 2.0 * margin).max(1.0));
            (room.w / drawn.w).min(room.h / drawn.h).min(1.0)
        }
    };
    let tx = (paper.w - drawn.w * scale) / 2.0;
    let ty = (paper.h - drawn.h * scale) / 2.0;
    let matrix = if turned {
        // Clockwise in y-down coordinates: (x, y) goes to (h - y, x), the top edge to the right.
        [0.0, scale, -scale, 0.0, scale * page.h + tx, ty]
    } else {
        [scale, 0.0, 0.0, scale, tx, ty]
    };
    Some(Placement { turned, scale, matrix })
}

/// Every page's placement, or the first page that is not the paper's size for [`Fit::Exact`]. Checked before
/// anything is sent, so a job never stops part-way for a page it cannot print.
pub(crate) fn place_all(pages: &[Size], paper: Size, fit: Fit) -> Result<Vec<Placement>, Mismatch> {
    pages.iter().enumerate().map(|(index, page)| place(*page, paper, fit).ok_or(Mismatch { index, page: *page })).collect()
}

/// `matrix` for content drawn `sx` and `sy` units to the DIP (a page rendered at a resolution): the scale back to
/// DIPs, then `matrix`.
pub(crate) fn after_scale(sx: f64, sy: f64, m: [f64; 6]) -> [f64; 6] {
    [sx * m[0], sx * m[1], sy * m[2], sy * m[3], m[4], m[5]]
}

/// The pages to draw, in order, for `copies` copies the job draws itself (the driver cannot make them as asked):
/// `Some(index)` a page, `None` a blank back.
///
/// Collated, the document repeats; after an odd page count on two-sided paper a blank back keeps the next copy
/// starting on a front. Uncollated, each sheet repeats: one page, or on two-sided paper a front and its back (the
/// last front alone gets a blank back between its copies).
pub(crate) fn sequence(pages: usize, copies: u32, collate: bool, two_sided: bool) -> Vec<Option<usize>> {
    let copies = copies.max(1) as usize;
    let mut out = Vec::new();
    if collate {
        let pad = two_sided && pages % 2 == 1;
        for copy in 0..copies {
            out.extend((0..pages).map(Some));
            if pad && copy + 1 < copies {
                out.push(None);
            }
        }
        return out;
    }
    let per_sheet = if two_sided { 2 } else { 1 };
    let mut first = 0;
    while first < pages {
        let sheet: Vec<Option<usize>> = (first..first + per_sheet).map(|i| (i < pages).then_some(i)).collect();
        for copy in 0..copies {
            let last = first + per_sheet >= pages && copy + 1 == copies;
            // The final sheet's blank back is left off its last copy: nothing follows it.
            let keep = if last { sheet.iter().rposition(Option::is_some).map_or(0, |i| i + 1) } else { sheet.len() };
            out.extend_from_slice(&sheet[..keep]);
        }
        first += per_sheet;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Size = Size::new(816.0, 1056.0);

    fn apply(m: [f64; 6], (x, y): (f64, f64)) -> (f64, f64) {
        (x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5])
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn a_page_the_papers_size_prints_as_it_is() {
        let p = place(LETTER, LETTER, Fit::Exact).unwrap();
        assert_eq!(p, Placement { turned: false, scale: 1.0, matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] });
        // Rounded metric sizes still match.
        assert!(place(Size::new(793.7, 1122.5), Size::from_micron(210_000, 297_000), Fit::Exact).is_some());
    }

    #[test]
    fn a_landscape_page_on_portrait_paper_turns_clockwise_onto_it() {
        let p = place(LETTER.turned(), LETTER, Fit::Exact).unwrap();
        assert!(p.turned);
        // The page's top-left corner goes to the paper's top-right, its top-right to the bottom-right.
        assert!(close(apply(p.matrix, (0.0, 0.0)), (816.0, 0.0)));
        assert!(close(apply(p.matrix, (1056.0, 0.0)), (816.0, 1056.0)));
        assert!(close(apply(p.matrix, (1056.0, 816.0)), (0.0, 1056.0)));
        // And a portrait page on landscape paper.
        let q = place(LETTER, LETTER.turned(), Fit::Exact).unwrap();
        assert!(q.turned);
        assert!(close(apply(q.matrix, (816.0, 1056.0)), (0.0, 816.0)));
    }

    #[test]
    fn exact_refuses_a_page_of_another_size_and_names_it() {
        let a4 = Size::from_micron(210_000, 297_000);
        assert_eq!(place(a4, LETTER, Fit::Exact), None);
        assert_eq!(place_all(&[LETTER, LETTER.turned(), a4], LETTER, Fit::Exact), Err(Mismatch { index: 2, page: a4 }));
        assert_eq!(place_all(&[LETTER, LETTER.turned()], LETTER, Fit::Exact).map(|p| p.len()), Ok(2));
    }

    #[test]
    fn shrink_fits_oversized_pages_inside_the_margin_and_keeps_small_ones() {
        let margin = 24.0;
        // Tabloid onto Letter: scaled to the printable area, centred.
        let tabloid = Size::new(1056.0, 1632.0);
        let p = place(tabloid, LETTER, Fit::Shrink { margin }).unwrap();
        let expected = ((816.0 - 48.0) / 1056.0_f64).min((1056.0 - 48.0) / 1632.0);
        assert!((p.scale - expected).abs() < 1e-12);
        let top_left = apply(p.matrix, (0.0, 0.0));
        let bottom_right = apply(p.matrix, (tabloid.w, tabloid.h));
        assert!(top_left.0 >= margin - 1e-9 && top_left.1 >= margin - 1e-9);
        assert!(bottom_right.0 <= 816.0 - margin + 1e-9 && bottom_right.1 <= 1056.0 - margin + 1e-9);
        assert!(close((top_left.0 + bottom_right.0, top_left.1 + bottom_right.1), (816.0, 1056.0)), "centred");
        // A small page keeps its size, centred.
        let card = Size::new(384.0, 576.0);
        let q = place(card, LETTER, Fit::Shrink { margin }).unwrap();
        assert_eq!(q.scale, 1.0);
        assert!(close(apply(q.matrix, (0.0, 0.0)), (216.0, 240.0)));
        // A landscape oversized page turns, then shrinks.
        assert!(place(tabloid.turned(), LETTER, Fit::Shrink { margin }).unwrap().turned);
        // A margin wider than the paper leaves a point to draw in rather than a negative scale.
        assert!(place(tabloid, LETTER, Fit::Shrink { margin: 1e6 }).unwrap().scale > 0.0);
        assert_eq!(place(card, LETTER, Fit::Shrink { margin: f64::NAN }).unwrap().scale, 1.0);
    }

    #[test]
    fn a_square_page_is_never_turned() {
        let square = Size::new(500.0, 500.0);
        assert!(!place(square, LETTER, Fit::Shrink { margin: 0.0 }).unwrap().turned);
        assert!(!place(square, LETTER.turned(), Fit::Shrink { margin: 0.0 }).unwrap().turned);
    }

    #[test]
    fn a_page_rendered_at_a_resolution_lands_where_its_dips_would() {
        let p = place(LETTER.turned(), LETTER, Fit::Shrink { margin: 10.0 }).unwrap();
        // Rendered at 600 dpi: 6.25 pixels to the DIP.
        let m = after_scale(1.0 / 6.25, 1.0 / 6.25, p.matrix);
        for (x, y) in [(0.0, 0.0), (1056.0, 0.0), (300.0, 700.0)] {
            assert!(close(apply(m, (x * 6.25, y * 6.25)), apply(p.matrix, (x, y))));
        }
    }

    #[test]
    fn collated_copies_drawn_here_repeat_the_document_and_keep_each_copy_on_a_front() {
        assert_eq!(sequence(2, 2, true, false), [Some(0), Some(1), Some(0), Some(1)]);
        assert_eq!(sequence(3, 2, true, false), [Some(0), Some(1), Some(2), Some(0), Some(1), Some(2)]);
        assert_eq!(sequence(3, 2, true, true), [Some(0), Some(1), Some(2), None, Some(0), Some(1), Some(2)], "a blank back between copies");
        assert_eq!(sequence(2, 2, true, true), [Some(0), Some(1), Some(0), Some(1)], "even counts need none");
        assert_eq!(sequence(3, 1, true, true), [Some(0), Some(1), Some(2)], "nothing after the last copy");
        assert_eq!(sequence(1, 0, true, false), [Some(0)], "at least one copy");
    }

    #[test]
    fn uncollated_copies_drawn_here_repeat_each_sheet() {
        assert_eq!(sequence(2, 2, false, false), [Some(0), Some(0), Some(1), Some(1)]);
        // Two-sided: a sheet is a front and its back.
        assert_eq!(sequence(4, 2, false, true), [Some(0), Some(1), Some(0), Some(1), Some(2), Some(3), Some(2), Some(3)]);
        assert_eq!(sequence(3, 2, false, true), [Some(0), Some(1), Some(0), Some(1), Some(2), None, Some(2)], "the last front alone");
        assert_eq!(sequence(0, 3, false, true), []);
    }
}

//! A fake `saned` on loopback for the tests. It has its own encoder and decoder, written apart from
//! the crate's, so a codec bug cannot pass on both sides.
//!
//! One device, `fake:0`, with the standard options: a source list, a mode list, a resolution word
//! list, a scan area in millimetres and a brightness range. A `Scene` says what the paper path
//! does (sheets in the feeder, a status at the n-th start, a slow data connection, three-pass
//! colour, a request for credentials); the `Log` records what the client asked for.

#![allow(dead_code)]

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const GOOD: i32 = 0;
pub const CANCELLED: i32 = 2;
pub const DEVICE_BUSY: i32 = 3;
pub const INVAL: i32 = 4;
pub const EOF: i32 = 5;
pub const JAMMED: i32 = 6;
pub const NO_DOCS: i32 = 7;
pub const COVER_OPEN: i32 = 8;

pub const DEVICE: &str = "fake:0";
pub const WIDTH: i32 = 16;
pub const LINES: i32 = 8;

/// What the fake device does.
#[derive(Clone, Debug)]
pub struct Scene {
    pub sources: Vec<&'static str>,
    pub modes: Vec<&'static str>,
    pub resolutions: Vec<i32>,
    /// Sheets in the feeder.
    pub sheets: usize,
    /// The status `open` answers.
    pub open_status: i32,
    /// `open` asks for credentials for this resource.
    pub auth_resource: Option<String>,
    /// The n-th start of a session (0-based, frames counted) answers this status.
    pub fail_at: Option<(usize, i32)>,
    /// The status byte that ends every frame's data.
    pub end_status: i32,
    /// Colour comes as three frames.
    pub three_pass: bool,
    /// The data trickles out, one line every 20 ms, over many lines.
    pub slow: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            sources: vec!["Flatbed", "ADF", "ADF Duplex"],
            modes: vec!["Lineart", "Gray", "Color"],
            resolutions: vec![75, 150, 300, 600],
            sheets: 0,
            open_status: GOOD,
            auth_resource: None,
            fail_at: None,
            end_status: EOF,
            three_pass: false,
            slow: false,
        }
    }
}

/// What the client did.
#[derive(Clone, Debug, Default)]
pub struct Log {
    /// Every procedure number received, in order.
    pub rpcs: Vec<i32>,
    /// `(option name, value as text)` for every set.
    pub sets: Vec<(String, String)>,
    /// User names sent with init.
    pub users: Vec<String>,
    /// Connections accepted on the control port.
    pub connections: usize,
}

/// The fake daemon; stops accepting when dropped.
pub struct FakeSaned {
    pub addr: SocketAddr,
    pub scene: Arc<Mutex<Scene>>,
    pub log: Arc<Mutex<Log>>,
    stop: Arc<AtomicBool>,
}

impl FakeSaned {
    pub fn start(scene: Scene) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let scene = Arc::new(Mutex::new(scene));
        let log = Arc::new(Mutex::new(Log::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, l, st) = (scene.clone(), log.clone(), stop.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                l.lock().unwrap().connections += 1;
                let (s, l) = (s.clone(), l.clone());
                thread::spawn(move || {
                    let _ = Session::new(stream, s, l).run();
                });
            }
        });
        Self { addr, scene, log, stop }
    }

    /// `127.0.0.1:<port>`.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    pub fn log(&self) -> Log {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FakeSaned {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

/// A reply being built.
#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn word(&mut self, w: i32) -> &mut Self {
        self.0.extend_from_slice(&w.to_be_bytes());
        self
    }
    fn string(&mut self, s: &str) -> &mut Self {
        self.word(s.len() as i32 + 1);
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        self
    }
}

fn fixed(mm: f64) -> i32 {
    (mm * 65536.0).round() as i32
}

/// One option of the fake.
struct Opt {
    name: &'static str,
    kind: i32,
    unit: i32,
    size: i32,
    cap: i32,
    /// 0 none, 1 range, 2 word list, 3 string list.
    constraint: i32,
}

const SOFT: i32 = 1 | 4;

fn options() -> Vec<Opt> {
    let o = |name, kind, unit, size, cap, constraint| Opt { name, kind, unit, size, cap, constraint };
    vec![
        o("", 1, 0, 4, 4, 0),
        o("", 5, 0, 0, 0, 0),
        o("source", 3, 0, 32, SOFT, 3),
        o("mode", 3, 0, 32, SOFT, 3),
        o("resolution", 1, 4, 4, SOFT, 2),
        o("tl-x", 2, 3, 4, SOFT, 1),
        o("tl-y", 2, 3, 4, SOFT, 1),
        o("br-x", 2, 3, 4, SOFT, 1),
        o("br-y", 2, 3, 4, SOFT, 1),
        o("brightness", 1, 0, 4, SOFT, 1),
    ]
}

/// One connection's state.
struct Session {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    scene: Arc<Mutex<Scene>>,
    log: Arc<Mutex<Log>>,
    source: String,
    mode: String,
    resolution: i32,
    area: [i32; 4],
    brightness: i32,
    starts: usize,
    frame: usize,
    /// The frame the last start began.
    current: usize,
    /// Sides left of the sheet in the feeder (duplex).
    sides_left: usize,
}

impl Session {
    fn new(stream: TcpStream, scene: Arc<Mutex<Scene>>, log: Arc<Mutex<Log>>) -> Self {
        let writer = stream.try_clone().unwrap();
        Self {
            reader: BufReader::new(stream),
            writer,
            scene,
            log,
            source: "Flatbed".into(),
            mode: "Gray".into(),
            resolution: 150,
            area: [0, 0, fixed(215.9), fixed(297.0)],
            brightness: 0,
            starts: 0,
            frame: 0,
            current: 0,
            sides_left: 0,
        }
    }

    fn scene(&self) -> Scene {
        self.scene.lock().unwrap().clone()
    }

    fn word(&mut self) -> std::io::Result<i32> {
        let mut b = [0; 4];
        self.reader.read_exact(&mut b)?;
        Ok(i32::from_be_bytes(b))
    }

    fn string(&mut self) -> std::io::Result<String> {
        let len = self.word()?;
        let mut b = vec![0; len.max(0) as usize];
        self.reader.read_exact(&mut b)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }

    fn send(&mut self, w: &W) -> std::io::Result<()> {
        self.writer.write_all(&w.0)
    }

    fn run(mut self) -> std::io::Result<()> {
        loop {
            let rpc = self.word()?;
            self.log.lock().unwrap().rpcs.push(rpc);
            match rpc {
                0 => {
                    let _version = self.word()?;
                    let user = self.string()?;
                    self.log.lock().unwrap().users.push(user);
                    self.send(W::default().word(GOOD).word(0x0100_0003))?;
                }
                1 => {
                    let mut w = W::default();
                    w.word(GOOD).word(2).word(1);
                    w.string(DEVICE).string("Fake").string("Flatbed ADF").string("flatbed scanner");
                    w.word(0);
                    self.send(&w)?;
                }
                2 => self.open()?,
                3 => {
                    self.word()?;
                    self.send(W::default().word(0))?;
                }
                4 => {
                    self.word()?;
                    let w = self.descriptors();
                    self.send(&w)?;
                }
                5 => self.control()?,
                6 => {
                    self.word()?;
                    let mut w = W::default();
                    w.word(GOOD);
                    self.parameters(&mut w);
                    self.send(&w)?;
                }
                7 => {
                    self.word()?;
                    self.start()?;
                }
                8 => {
                    self.word()?;
                    self.frame = 0;
                    self.send(W::default().word(0))?;
                }
                9 => {
                    let (_r, _u, _p) = (self.string()?, self.string()?, self.string()?);
                    self.send(W::default().word(0))?;
                }
                10 => return Ok(()),
                _ => return Ok(()),
            }
        }
    }

    fn open(&mut self) -> std::io::Result<()> {
        let name = self.string()?;
        let scene = self.scene();
        let mut w = W::default();
        if let Some(resource) = scene.auth_resource {
            w.word(GOOD).word(0).string(&resource);
        } else if name != DEVICE {
            w.word(INVAL).word(0).string("");
        } else {
            w.word(scene.open_status).word(1).string("");
        }
        self.send(&w)
    }

    fn descriptors(&self) -> W {
        let scene = self.scene();
        let opts = options();
        let mut w = W::default();
        w.word(opts.len() as i32);
        for o in &opts {
            w.word(1).string(o.name).string(o.name).string("");
            w.word(o.kind).word(o.unit).word(o.size).word(o.cap).word(o.constraint);
            match (o.constraint, o.name) {
                (3, "source") => {
                    w.word(scene.sources.len() as i32 + 1);
                    for s in &scene.sources {
                        w.string(s);
                    }
                    w.word(0);
                }
                (3, _) => {
                    w.word(scene.modes.len() as i32 + 1);
                    for m in &scene.modes {
                        w.string(m);
                    }
                    w.word(0);
                }
                (2, _) => {
                    w.word(scene.resolutions.len() as i32 + 1).word(scene.resolutions.len() as i32);
                    for r in &scene.resolutions {
                        w.word(*r);
                    }
                }
                (1, "brightness") => {
                    w.word(1).word(-100).word(100).word(1);
                }
                (1, name) => {
                    // The feeder's sheets are longer than the glass.
                    let max = if name.ends_with('x') {
                        215.9
                    } else if self.source == "Flatbed" {
                        297.0
                    } else {
                        355.6
                    };
                    w.word(1).word(0).word(fixed(max)).word(0);
                }
                _ => {}
            }
        }
        w
    }

    fn control(&mut self) -> std::io::Result<()> {
        let _h = self.word()?;
        let index = self.word()? as usize;
        let action = self.word()?;
        let kind = self.word()?;
        let size = self.word()?;
        let count = self.word()?;
        let (text, words) = if kind == 3 {
            let mut b = vec![0; count.max(0) as usize];
            self.reader.read_exact(&mut b)?;
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            (String::from_utf8_lossy(&b[..end]).into_owned(), Vec::new())
        } else {
            let mut words = Vec::new();
            for _ in 0..count {
                words.push(self.word()?);
            }
            (String::new(), words)
        };
        let opts = options();
        let name = opts.get(index).map_or("", |o| o.name);
        let scene = self.scene();
        let mut status = GOOD;
        let mut info = 0;
        if action == 1 {
            self.log.lock().unwrap().sets.push((name.to_owned(), if kind == 3 { text.clone() } else { format!("{:?}", words) }));
            match name {
                "source" if scene.sources.iter().any(|s| *s == text) => {
                    self.source = text.clone();
                    info = 2;
                }
                "mode" if scene.modes.iter().any(|m| *m == text) => {
                    self.mode = text.clone();
                    info = 2 | 4;
                }
                "resolution" if scene.resolutions.contains(&words[0]) => self.resolution = words[0],
                "tl-x" => self.area[0] = words[0],
                "tl-y" => self.area[1] = words[0],
                "br-x" => self.area[2] = words[0],
                "br-y" => self.area[3] = words[0],
                "brightness" => self.brightness = words[0],
                _ => status = INVAL,
            }
        }
        let mut w = W::default();
        w.word(status).word(info).word(kind).word(size);
        match name {
            "source" | "mode" => {
                let current = if name == "source" { self.source.clone() } else { self.mode.clone() };
                let mut buf = current.into_bytes();
                buf.resize(size as usize, 0);
                w.word(size);
                w.0.extend_from_slice(&buf);
            }
            "resolution" => {
                w.word(1).word(self.resolution);
            }
            _ => {
                w.word(1).word(0);
            }
        }
        w.string("");
        self.send(&w)
    }

    /// `(format, last, bytes per line, depth)` of the current frame.
    fn shape(&self) -> (i32, bool, i32, i32) {
        let three = self.scene().three_pass;
        match self.mode.as_str() {
            "Lineart" => (0, true, WIDTH / 8, 1),
            "Color" if three => (2 + self.current as i32, self.current == 2, WIDTH, 8),
            "Color" => (1, true, WIDTH * 3, 8),
            _ => (0, true, WIDTH, 8),
        }
    }

    fn lines(&self) -> i32 {
        if self.scene().slow { 400 } else { LINES }
    }

    fn parameters(&self, w: &mut W) {
        let (format, last, bpl, depth) = self.shape();
        w.word(format).word(i32::from(last)).word(bpl).word(WIDTH).word(self.lines()).word(depth);
    }

    fn start(&mut self) -> std::io::Result<()> {
        let scene = self.scene();
        let index = self.starts;
        self.starts += 1;
        let refuse = |s: &mut Self, status: i32| s.send(W::default().word(status).word(0).word(0x1234).string(""));
        if let Some((at, status)) = scene.fail_at
            && index >= at
        {
            return refuse(self, status);
        }
        // A new sheet only for the first frame of an image.
        if self.frame == 0 && self.source != "Flatbed" {
            if self.sides_left == 0 {
                let mut s = self.scene.lock().unwrap();
                if s.sheets == 0 {
                    drop(s);
                    return refuse(self, NO_DOCS);
                }
                s.sheets -= 1;
                self.sides_left = if self.source.contains("Duplex") { 2 } else { 1 };
            }
            self.sides_left -= 1;
        }
        self.current = self.frame;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_, last, bpl, _) = self.shape();
        let lines = self.lines();
        let channel = self.current as u8;
        let slow = scene.slow;
        let end = scene.end_status;
        thread::spawn(move || {
            let Ok((mut data, _)) = listener.accept() else { return };
            let row = |y: i32| -> Vec<u8> { (0..bpl).map(|x| (x + y * 3) as u8 ^ (channel * 40)).collect() };
            let mut ok = true;
            if slow {
                for y in 0..lines {
                    let r = row(y);
                    let mut rec = (r.len() as u32).to_be_bytes().to_vec();
                    rec.extend_from_slice(&r);
                    if data.write_all(&rec).is_err() {
                        ok = false;
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            } else {
                // Two records, split mid-line.
                let all: Vec<u8> = (0..lines).flat_map(row).collect();
                let (a, b) = all.split_at(all.len() / 2 + 1);
                for part in [a, b] {
                    let mut rec = (part.len() as u32).to_be_bytes().to_vec();
                    rec.extend_from_slice(part);
                    ok &= data.write_all(&rec).is_ok();
                }
            }
            if ok {
                let _ = data.write_all(&[0xff, 0xff, 0xff, 0xff, end as u8]);
            }
            let _ = data.shutdown(Shutdown::Write);
        });
        self.frame = if last { 0 } else { self.frame + 1 };
        self.send(W::default().word(GOOD).word(i32::from(port)).word(0x1234).string(""))
    }
}

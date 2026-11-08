//! A client of one `saned`: a control connection with one method per remote procedure, and the
//! data connection of a started frame.
//!
//! A daemon serves one client per connection, and the connection ends with [`Client::exit`]. Calls
//! block, bounded by [`Timeouts`]; replies are bounded by [`Limits`]. After a transport or protocol
//! error, or a reply that asks for authorization, the connection is out of step and the client
//! refuses further calls ([`Client::is_broken`]); drop it and connect again.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use crate::error::{Error, Result, TransportKind};
use crate::policy::SaneHost;
use crate::protocol::{
    self, ByteOrder, Device, NET_PROTOCOL_VERSION, OptionDescriptor, OptionTable, Parameters, Procedure, SANE_MAJOR, Status, Value, ValueType,
    version_code, version_parts,
};
use crate::wire::{Decoder, Encoder, Limits};

/// How long the client waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Timeouts {
    /// For a connection to be made (default 5 s).
    pub connect: Duration,
    /// For a reply on the control connection (default 60 s: opening a device can take a while).
    pub reply: Duration,
    /// Between two pieces of image data (default 120 s: a lamp warms up, a feeder pulls a sheet).
    pub data: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { connect: Duration::from_secs(5), reply: Duration::from_secs(60), data: Duration::from_secs(120) }
    }
}

setters!(Timeouts {
    with_connect => connect: Duration,
    with_reply => reply: Duration,
    with_data => data: Duration,
});

/// How a client connects.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientOptions {
    /// The user name sent with init (default `printscan`). The daemon uses it only to look up
    /// credentials, which this client never sends.
    pub user_name: String,
    pub timeouts: Timeouts,
    pub limits: Limits,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self { user_name: "printscan".into(), timeouts: Timeouts::default(), limits: Limits::default() }
    }
}

setters!(ClientOptions {
    with_user_name => user_name: String,
    with_timeouts => timeouts: Timeouts,
    with_limits => limits: Limits,
});

/// An open device on the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle(i32);

impl Handle {
    /// The daemon's number for the handle.
    pub const fn word(self) -> i32 {
        self.0
    }
}

/// What setting or reading an option returned.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ControlReply {
    /// [`protocol::info`] bits.
    pub info: i32,
    /// The value the option has now.
    pub value: Value,
}

impl ControlReply {
    /// The descriptors changed: read them again.
    pub const fn reload_options(&self) -> bool {
        self.info & protocol::info::RELOAD_OPTIONS != 0
    }

    /// The driver rounded the value.
    pub const fn inexact(&self) -> bool {
        self.info & protocol::info::INEXACT != 0
    }
}

/// What a start returned: where the frame's data is, and the order of its 16-bit samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Started {
    pub port: u16,
    pub byte_order: ByteOrder,
}

/// The action word of a control-option request.
#[derive(Clone, Copy)]
enum Action {
    Get = 0,
    Set = 1,
    Auto = 2,
}

/// Open a TCP connection to `addr` with the connect timeout, and the given I/O timeout.
fn dial(addr: SocketAddr, connect: Duration, io: Duration, what: &str) -> Result<TcpStream> {
    // A connect that failed sent nothing, whatever the reason; only a timeout is told apart.
    let stream = TcpStream::connect_timeout(&addr, connect).map_err(|e| {
        let kind = if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
            TransportKind::ConnectTimeout
        } else {
            TransportKind::Refused
        };
        Error::transport(kind, format!("connecting to the SANE {what} at {addr}: {e}"))
    })?;
    let timeout = Some(io.max(Duration::from_millis(1)));
    let configured = stream.set_read_timeout(timeout).and_then(|()| stream.set_write_timeout(timeout)).and_then(|()| stream.set_nodelay(true));
    configured.map_err(|e| Error::io("configuring the connection", &e))?;
    Ok(stream)
}

/// An authorization request: the resource, without the salt some daemons append after `$MD5$`.
fn authorization(resource: &str) -> Error {
    let shown = resource.split("$MD5$").next().unwrap_or(resource);
    Error::AuthorizationRequired { resource: shown.to_owned() }
}

/// `status` as the result of `operation`.
fn check(operation: &'static str, status: Status) -> Result<()> {
    if status == Status::Good { Ok(()) } else { Err(Error::Status { operation, status }) }
}

/// The control connection to one daemon.
#[derive(Debug)]
pub struct Client {
    host: SaneHost,
    reader: Decoder<BufReader<TcpStream>>,
    writer: TcpStream,
    options: ClientOptions,
    version: i32,
    broken: bool,
}

impl Client {
    /// Connect to `host` and run init. Fails when the daemon refuses init or speaks another major
    /// version of SANE.
    pub fn connect(host: SaneHost, options: &ClientOptions) -> Result<Self> {
        let stream = dial(host.socket_addr(), options.timeouts.connect, options.timeouts.reply, "daemon")?;
        let writer = stream.try_clone().map_err(|e| Error::io("cloning the connection", &e))?;
        let reader = Decoder::new(BufReader::new(stream), options.limits);
        let mut client = Self { host, reader, writer, options: options.clone(), version: 0, broken: false };
        let mut e = Encoder::new();
        e.word(Procedure::Init.code()).word(version_code(SANE_MAJOR, 0, NET_PROTOCOL_VERSION));
        e.string(Some(&options.user_name))?;
        let (status, version) = client.exchange(&e, |d| Ok((protocol::read_status(d)?, d.word()?)))?;
        check("init", status)?;
        let (major, _, _) = version_parts(version);
        if major != SANE_MAJOR {
            client.broken = true;
            return Err(Error::Protocol(format!("the daemon speaks SANE {major}, not {SANE_MAJOR}")));
        }
        client.version = version;
        Ok(client)
    }

    /// The daemon this client talks to.
    pub fn host(&self) -> SaneHost {
        self.host
    }

    /// The daemon's version code from init: `(major, minor, build)` by [`protocol::version_parts`].
    pub fn daemon_version(&self) -> i32 {
        self.version
    }

    /// Whether the connection is out of step and no further call will be made.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Send `request` and read the reply with `read`. Any failure but a status the reply carries
    /// leaves the connection broken.
    fn exchange<T>(&mut self, request: &Encoder, read: impl FnOnce(&mut Decoder<BufReader<TcpStream>>) -> Result<T>) -> Result<T> {
        if self.broken {
            return Err(Error::transport(TransportKind::Closed, "the connection to the SANE daemon is no longer usable"));
        }
        let sent = self.writer.write_all(request.as_bytes()).and_then(|()| self.writer.flush());
        if let Err(e) = sent {
            self.broken = true;
            return Err(Error::io("sending to the SANE daemon", &e));
        }
        let result = read(&mut self.reader);
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    /// A request of `procedure` with one handle word.
    fn with_handle(procedure: Procedure, handle: Handle) -> Encoder {
        let mut e = Encoder::new();
        e.word(procedure.code()).word(handle.0);
        e
    }

    /// The devices the daemon offers.
    pub fn devices(&mut self) -> Result<Vec<Device>> {
        let mut e = Encoder::new();
        e.word(Procedure::GetDevices.code());
        let (status, devices) = self.exchange(&e, |d| Ok((protocol::read_status(d)?, protocol::read_devices(d)?)))?;
        check("get devices", status)?;
        Ok(devices)
    }

    /// Open the device named `name`. A daemon that asks for credentials is
    /// [`Error::AuthorizationRequired`], and the connection is closed.
    pub fn open(&mut self, name: &str) -> Result<Handle> {
        let mut e = Encoder::new();
        e.word(Procedure::Open.code());
        e.string(Some(name))?;
        let (status, handle, resource) = self.exchange(&e, |d| Ok((protocol::read_status(d)?, d.word()?, d.text()?)))?;
        if !resource.is_empty() {
            self.broken = true;
            return Err(authorization(&resource));
        }
        check("open", status)?;
        Ok(Handle(handle))
    }

    /// Close an open device.
    pub fn close(&mut self, handle: Handle) -> Result<()> {
        self.exchange(&Self::with_handle(Procedure::Close, handle), |d| d.word().map(drop))
    }

    /// The device's option descriptors.
    pub fn option_descriptors(&mut self, handle: Handle) -> Result<OptionTable> {
        self.exchange(&Self::with_handle(Procedure::GetOptionDescriptors, handle), protocol::read_option_table)
    }

    fn control(&mut self, handle: Handle, index: usize, action: Action, desc: &OptionDescriptor, value: &Value) -> Result<ControlReply> {
        let mut e = Self::with_handle(Procedure::ControlOption, handle);
        e.count(index)?;
        e.word(action as i32).word(desc.value_type.code()).word(desc.size);
        protocol::write_value(&mut e, desc.value_type, desc.size, value)?;
        let (status, info, value, resource) = self.exchange(&e, |d| {
            let status = protocol::read_status(d)?;
            let info = d.word()?;
            let value_type = ValueType::from_word(d.word()?);
            let _size = d.word()?;
            let value = protocol::read_value(d, value_type)?;
            Ok((status, info, value, d.text()?))
        })?;
        if !resource.is_empty() {
            self.broken = true;
            return Err(authorization(&resource));
        }
        check("control option", status)?;
        Ok(ControlReply { info, value })
    }

    /// An empty buffer of the option's type and size, for a get.
    fn buffer(desc: &OptionDescriptor) -> Value {
        match desc.value_type {
            ValueType::String => Value::String(String::new()),
            ValueType::Bool | ValueType::Int | ValueType::Fixed => Value::Words(vec![0; desc.word_count()]),
            _ => Value::None,
        }
    }

    /// The value of option `index`, described by `desc`.
    pub fn get_option(&mut self, handle: Handle, index: usize, desc: &OptionDescriptor) -> Result<Value> {
        Ok(self.control(handle, index, Action::Get, desc, &Self::buffer(desc))?.value)
    }

    /// Set option `index` to `value`, checked against `desc`'s type and size before sending.
    pub fn set_option(&mut self, handle: Handle, index: usize, desc: &OptionDescriptor, value: &Value) -> Result<ControlReply> {
        self.control(handle, index, Action::Set, desc, value)
    }

    /// Let the driver choose option `index` itself.
    pub fn set_auto(&mut self, handle: Handle, index: usize, desc: &OptionDescriptor) -> Result<ControlReply> {
        self.control(handle, index, Action::Auto, desc, &Self::buffer(desc))
    }

    /// The parameters of the next frame (exact once it started).
    pub fn parameters(&mut self, handle: Handle) -> Result<Parameters> {
        let (status, parameters) =
            self.exchange(&Self::with_handle(Procedure::GetParameters, handle), |d| Ok((protocol::read_status(d)?, protocol::read_parameters(d)?)))?;
        check("get parameters", status)?;
        Ok(parameters)
    }

    /// Start the next frame. An empty feeder is [`Error::Status`] with [`Status::NoDocs`].
    pub fn start(&mut self, handle: Handle) -> Result<Started> {
        let (status, port, order, resource) =
            self.exchange(&Self::with_handle(Procedure::Start, handle), |d| Ok((protocol::read_status(d)?, d.word()?, d.word()?, d.text()?)))?;
        if !resource.is_empty() {
            self.broken = true;
            return Err(authorization(&resource));
        }
        check("start", status)?;
        let port = u16::try_from(port).map_err(|_| Error::Protocol(format!("{port} is not a port")))?;
        let byte_order = ByteOrder::from_word(order).inspect_err(|_| self.broken = true)?;
        Ok(Started { port, byte_order })
    }

    /// Connect to a started frame's data, on the control connection's address.
    pub fn data(&self, started: &Started) -> Result<DataStream> {
        let addr = self.host.data_addr(started.port)?;
        let stream = dial(addr, self.options.timeouts.connect, self.options.timeouts.data, "data port")?;
        Ok(DataStream { stream, limits: self.options.limits, byte_order: started.byte_order })
    }

    /// End the scan in progress, or reset the device after the last frame.
    pub fn cancel(&mut self, handle: Handle) -> Result<()> {
        self.exchange(&Self::with_handle(Procedure::Cancel, handle), |d| d.word().map(drop))
    }

    /// End the session: exit, then close the connection. Best effort; a broken connection is
    /// only closed.
    pub fn exit(mut self) {
        if !self.broken {
            let mut e = Encoder::new();
            e.word(Procedure::Exit.code());
            let _ = self.writer.write_all(e.as_bytes()).and_then(|()| self.writer.flush());
        }
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// The data connection of one started frame: records of a length word and that many bytes, ended by
/// the length `0xFFFFFFFF` and one status byte.
#[derive(Debug)]
pub struct DataStream {
    stream: TcpStream,
    limits: Limits,
    byte_order: ByteOrder,
}

impl DataStream {
    /// A second handle on the connection, so another thread can shut it down to stop a transfer.
    pub fn shutdown_handle(&self) -> Option<TcpStream> {
        self.stream.try_clone().ok()
    }

    /// The order of the frame's 16-bit samples.
    pub fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }

    /// Read the frame to its end: the bytes, and the status that ended it ([`Status::Eof`] for a
    /// complete frame). `progress` is called with the bytes received so far after every record. A
    /// frame larger than [`Limits::max_image`] is [`Error::TooLarge`]; a connection that closes
    /// before the end marker is a [`TransportKind::Closed`] error.
    pub fn read_frame(&mut self, progress: &mut dyn FnMut(u64)) -> Result<(Vec<u8>, Status)> {
        let mut data = Vec::new();
        loop {
            let mut word = [0; 4];
            self.stream.read_exact(&mut word).map_err(|e| Error::io("reading image data", &e))?;
            let len = u32::from_be_bytes(word);
            if len == u32::MAX {
                let mut status = [0; 1];
                self.stream.read_exact(&mut status).map_err(|e| Error::io("reading the end of the image data", &e))?;
                return Ok((data, Status::from_word(i32::from(status[0]))));
            }
            let total = data.len() as u64 + u64::from(len);
            if total > self.limits.max_image {
                return Err(Error::TooLarge { what: "the image data", limit: self.limits.max_image });
            }
            let start = data.len();
            data.resize(start + len as usize, 0);
            self.stream.read_exact(&mut data[start..]).map_err(|e| Error::io("reading image data", &e))?;
            progress(total);
        }
    }
}

//! A minimal blocking Rust client for `tinytrader.gateway/1`.
use crate::protocol::Obj;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// One connection: `hello` is sent on connect; every other line is read raw.
pub struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    /// Connects and sends `hello`.
    ///
    /// # Errors
    /// Connection or write failure.
    pub fn connect(
        addr: SocketAddr,
        client: &str,
        token: &str,
        resume_after_seq: Option<u64>,
        cancel_on_disconnect: bool,
    ) -> io::Result<Self> {
        let w = TcpStream::connect(addr)?;
        w.set_read_timeout(Some(Duration::from_secs(20)))?;
        let r = BufReader::new(w.try_clone()?);
        let mut c = Self { w, r };
        let hello = Obj::new()
            .s("client", client)
            .s("token", token)
            .raw(
                "resume_after_seq",
                &resume_after_seq.map_or("null".into(), |n| n.to_string()),
            )
            .b("cancel_on_disconnect", cancel_on_disconnect)
            .done();
        c.send("hello", &hello)?;
        Ok(c)
    }

    /// Sends `{"schema", "type": ty, ...fields}`; `fields` is a JSON object text.
    ///
    /// # Errors
    /// Write failure.
    pub fn send(&mut self, ty: &str, fields: &str) -> io::Result<()> {
        let line = Obj::msg(ty).merge(fields).done();
        self.send_raw(line.as_bytes())
    }

    /// Writes bytes followed by a newline.
    ///
    /// # Errors
    /// Write failure.
    pub fn send_raw(&mut self, line: &[u8]) -> io::Result<()> {
        self.w.write_all(line)?;
        self.w.write_all(b"\n")
    }

    /// The next line without its newline; `None` at end of stream.
    ///
    /// # Errors
    /// Read failure or timeout.
    pub fn line(&mut self) -> io::Result<Option<String>> {
        let mut s = String::new();
        if self.r.read_line(&mut s)? == 0 {
            return Ok(None);
        }
        Ok(Some(s.trim_end_matches('\n').to_owned()))
    }

    /// Sets the read timeout.
    ///
    /// # Errors
    /// Socket failure.
    pub fn set_timeout(&self, t: Duration) -> io::Result<()> {
        self.w.set_read_timeout(Some(t))
    }
}

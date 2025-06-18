use crate::{api, channel, service};

use std::{
    io::{self, Write},
    sync,
};

enum RdpStreamState {
    Ready,
    Connected,
    Disconnected,
}

impl RdpStreamState {
    const fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

struct RdpStreamCommon<'a> {
    channel: &'a channel::Channel,
    service: &'a service::Service,
    client_id: api::ClientId,
    state: RdpStreamState,
}

impl RdpStreamCommon<'_> {
    #[cfg(feature = "backend")]
    fn accept(&mut self) -> Result<(), io::Error> {
        match &self.state {
            RdpStreamState::Ready => {
                crate::debug!("{} accept {:x}", self.service, self.client_id);
                self.state = RdpStreamState::Connected;
                Ok(())
            }
            RdpStreamState::Connected => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "already connected",
            )),
            RdpStreamState::Disconnected => {
                Err(io::Error::new(io::ErrorKind::Interrupted, "disconnected"))
            }
        }
    }

    #[cfg(feature = "frontend")]
    fn connect(&mut self) -> Result<(), io::Error> {
        match &self.state {
            RdpStreamState::Ready => {
                self.channel
                    .send_chunk(api::Chunk::start(self.client_id, self.service)?)
                    .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?;
                crate::debug!("connect",);
                self.state = RdpStreamState::Connected;
                Ok(())
            }
            RdpStreamState::Connected => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "already connected",
            )),
            RdpStreamState::Disconnected => {
                Err(io::Error::new(io::ErrorKind::Interrupted, "disconnected"))
            }
        }
    }

    fn disconnected(&mut self) {
        crate::debug!("disconnected",);
        self.channel.forget(self.client_id);
        self.state = RdpStreamState::Disconnected;
    }

    fn disconnect(&mut self) {
        match &self.state {
            RdpStreamState::Ready => {
                self.disconnected();
            }
            RdpStreamState::Connected => {
                crate::debug!("disconnecting",);
                let _ = self.channel.send_chunk(api::Chunk::end(self.client_id));
                self.disconnected();
            }
            RdpStreamState::Disconnected => (),
        }
    }
}

impl Drop for RdpStreamCommon<'_> {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[derive(Clone)]
struct RdpStreamControl<'a>(sync::Arc<sync::RwLock<RdpStreamCommon<'a>>>);

impl<'a> RdpStreamControl<'a> {
    fn new(
        channel: &'a channel::Channel,
        service: &'a service::Service,
        client_id: api::ClientId,
    ) -> Self {
        Self(sync::Arc::new(sync::RwLock::new(RdpStreamCommon {
            channel,
            service,
            client_id,
            state: RdpStreamState::Ready,
        })))
    }

    fn client_id(&self) -> api::ClientId {
        self.0.read().unwrap().client_id
    }

    fn is_connected(&self) -> bool {
        self.0.read().unwrap().state.is_connected()
    }

    #[cfg(feature = "backend")]
    fn accept(&self) -> Result<(), io::Error> {
        self.0.write().unwrap().accept()
    }

    #[cfg(feature = "frontend")]
    fn connect(&self) -> Result<(), io::Error> {
        self.0.write().unwrap().connect()
    }

    fn send_chunk(&self, chunk: api::Chunk) -> Result<(), io::Error> {
        self.0
            .write()
            .unwrap()
            .channel
            .send_chunk(chunk)
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))
    }

    fn disconnected(&self) {
        self.0.write().unwrap().disconnected();
    }

    fn disconnect(&self) {
        self.0.write().unwrap().disconnect();
    }
}

pub struct RdpStream<'a> {
    reader: RdpReader<'a>,
    writer: RdpWriter<'a>,
    control: RdpStreamControl<'a>,
}

impl<'a> RdpStream<'a> {
    pub(crate) fn new(
        channel: &'a channel::Channel,
        service: &'a service::Service,
        client_id: api::ClientId,
        from_rdp: crossbeam_channel::Receiver<api::Chunk>,
    ) -> Self {
        let control = RdpStreamControl::new(channel, service, client_id);

        let reader = RdpReader::new(control.clone(), from_rdp);
        let writer = RdpWriter::new(control.clone());

        Self {
            reader,
            writer,
            control,
        }
    }

    pub(crate) fn client_id(&self) -> api::ClientId {
        self.control.client_id()
    }

    #[cfg(feature = "backend")]
    pub(crate) fn accept(&self) -> Result<(), io::Error> {
        self.control.accept()
    }

    #[cfg(feature = "frontend")]
    pub(crate) fn connect(&self) -> Result<(), io::Error> {
        self.control.connect()
    }

    pub(crate) fn disconnect(&mut self) -> Result<(), io::Error> {
        self.writer.flush()?;
        self.control.disconnect();
        Ok(())
    }

    pub(crate) fn split(self) -> (RdpReader<'a>, RdpWriter<'a>) {
        (self.reader, self.writer)
    }
}

impl io::Read for RdpStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.reader.read(buf)
    }
}

impl io::Write for RdpStream<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        self.writer.flush()
    }
}

pub(crate) struct RdpReader<'a> {
    control: RdpStreamControl<'a>,
    from_rdp: crossbeam_channel::Receiver<api::Chunk>,
    last: Option<(api::Chunk, usize)>,
}

impl<'a> RdpReader<'a> {
    const fn new(
        control: RdpStreamControl<'a>,
        from_rdp: crossbeam_channel::Receiver<api::Chunk>,
    ) -> Self {
        Self {
            control,
            from_rdp,
            last: None,
        }
    }

    pub(crate) fn disconnect(&self) {
        self.control.disconnect();
    }
}

impl io::Read for RdpReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        if !self.control.is_connected() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "ended"));
        }

        if self.last.is_none() {
            let chunk = self
                .from_rdp
                .recv()
                .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;
            let chunk_type = chunk.chunk_type();
            let payload = chunk.payload();
            let payload_len = payload.len();
            if matches!(chunk_type, Ok(api::ChunkType::End)) {
                self.control.disconnected();
                return Ok(0);
            }
            if payload_len == 0 {
                return Ok(0);
            }
            if payload_len <= buf.len() {
                buf[0..payload_len].copy_from_slice(payload);
                return Ok(payload_len);
            }
            self.last = Some((chunk, 0));
        }

        let (last, last_offset) = self.last.as_mut().unwrap();
        let last_payload = last.payload();
        let last_payload_len = last_payload.len();
        let last_len = last_payload_len - *last_offset;
        let buf_len = buf.len();

        if last_len <= buf_len {
            buf[0..last_len].copy_from_slice(&last_payload[*last_offset..]);
            self.last = None;
            return Ok(last_len);
        }

        buf.copy_from_slice(&last_payload[*last_offset..*last_offset + buf_len]);
        *last_offset += buf_len;

        Ok(buf_len)
    }
}

#[derive(Clone)]
pub(crate) struct RdpWriter<'a> {
    control: RdpStreamControl<'a>,
    buffer: [u8; api::Chunk::max_payload_length()],
    buffer_len: usize,
}

impl<'a> RdpWriter<'a> {
    const fn new(control: RdpStreamControl<'a>) -> Self {
        Self {
            control,
            buffer: [0u8; api::Chunk::max_payload_length()],
            buffer_len: 0,
        }
    }

    pub(crate) fn disconnect(&mut self) -> Result<(), io::Error> {
        self.flush()?;
        self.control.disconnect();
        Ok(())
    }
}

impl Drop for RdpWriter<'_> {
    fn drop(&mut self) {
        let _ = self.flush();
        let _ = self.buffer;
        let _ = self.control;
    }
}

impl io::Write for RdpWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        if !self.control.is_connected() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "ended"));
        }

        let buf_len = buf.len();
        let remaining_len = self.buffer.len() - self.buffer_len;

        if buf_len <= remaining_len {
            self.buffer[self.buffer_len..(self.buffer_len + buf_len)].copy_from_slice(buf);
            self.buffer_len += buf_len;
            if self.buffer.len() == self.buffer_len {
                self.flush()?;
            }
            Ok(buf_len)
        } else {
            self.buffer[self.buffer_len..].copy_from_slice(&buf[0..remaining_len]);
            self.buffer_len += remaining_len;

            self.flush()?;

            if remaining_len < buf_len {
                let len = usize::min(buf_len - remaining_len, self.buffer.len());
                self.buffer[0..len].copy_from_slice(&buf[remaining_len..(remaining_len + len)]);
                self.buffer_len = len;
                Ok(remaining_len + len)
            } else {
                Ok(remaining_len)
            }
        }
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        if 0 < self.buffer_len {
            let chunk =
                api::Chunk::data(self.control.client_id(), &self.buffer[0..self.buffer_len])?;
            self.buffer_len = 0;

            if let Err(e) = self.control.send_chunk(chunk) {
                self.control.disconnected();
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, e));
            }
        }
        Ok(())
    }
}

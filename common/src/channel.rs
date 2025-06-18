#[cfg(feature = "frontend")]
use crate::input;
use crate::{api, rdp, service};

#[cfg(feature = "backend")]
use std::collections::hash_map;
use std::{collections, io, sync, thread};

pub(crate) const CLIENT_CHUNK_BUFFER_SIZE: usize = 16;

pub struct Channel {
    clients:
        sync::RwLock<collections::HashMap<api::ClientId, crossbeam_channel::Sender<api::Chunk>>>,
    to_rdp: crossbeam_channel::Sender<api::ChannelControl>,
}

impl Channel {
    pub fn new(to_rdp: crossbeam_channel::Sender<api::ChannelControl>) -> Self {
        Self {
            clients: sync::RwLock::new(collections::HashMap::new()),
            to_rdp,
        }
    }

    pub(crate) fn shutdown(&self) {
        match self.clients.write() {
            sync::LockResult::Err(e) => {
                crate::error!("failed to acquire lock to shutdown channel: {e}");
            }
            sync::LockResult::Ok(mut clients) => {
                clients.iter().for_each(|(client_id, client)| {
                    let _ = client.send(api::Chunk::end(*client_id));
                });
                clients.clear();
            }
        }
    }

    pub(crate) fn forget(&self, client_id: api::ClientId) {
        let _ = self.clients.write().unwrap().remove(&client_id);
    }

    pub(crate) fn send_chunk(&self, chunk: api::Chunk) -> Result<(), api::Error> {
        self.to_rdp.send(api::ChannelControl::SendChunk(chunk))?;
        Ok(())
    }

    #[cfg(feature = "frontend")]
    pub(crate) fn reset_client(&self) -> Result<(), api::Error> {
        self.to_rdp.send(api::ChannelControl::ResetClient)?;
        Ok(())
    }

    #[cfg(feature = "frontend")]
    pub(crate) fn send_input_setting(
        &self,
        setting: input::InputSetting,
    ) -> Result<(), api::Error> {
        self.to_rdp
            .send(api::ChannelControl::SendInputSetting(setting))?;
        Ok(())
    }

    #[cfg(feature = "frontend")]
    pub(crate) fn send_input_action(&self, action: input::InputAction) -> Result<(), api::Error> {
        self.to_rdp
            .send(api::ChannelControl::SendInputAction(action))?;
        Ok(())
    }

    #[cfg(feature = "frontend")]
    pub(crate) fn connect<'a>(
        &'a self,
        service: &'a service::Service,
    ) -> Result<rdp::RdpStream<'a>, io::Error> {
        let client_id = api::new_client_id();

        let (from_rdp_send, from_rdp_recv) = crossbeam_channel::bounded(CLIENT_CHUNK_BUFFER_SIZE);

        self.clients
            .write()
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?
            .insert(client_id, from_rdp_send);

        let stream = rdp::RdpStream::new(self, service, client_id, from_rdp_recv);
        match stream.connect() {
            Err(e) => {
                self.forget(client_id);
                Err(e)
            }
            Ok(()) => Ok(stream),
        }
    }

    #[cfg(feature = "backend")]
    fn handle_backend_start<'a>(
        &'a self,
        client_id: api::ClientId,
        payload: &[u8],
        scope: &'a thread::Scope<'a, '_>,
    ) -> Result<(), api::Error> {
        match self
            .clients
            .write()
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?
            .entry(client_id)
        {
            hash_map::Entry::Occupied(_) => {
                crate::error!("discarding start for already existing client {client_id:x}");
            }
            hash_map::Entry::Vacant(ve) => match service::lookup_bytes(payload) {
                Err(service) => {
                    crate::error!("new client for unknown service {service}!");
                    self.send_chunk(api::Chunk::end(client_id))?;
                }
                Ok(service) => {
                    crate::debug!("new {service} client {client_id:x}");

                    match &service.backend {
                        None => {
                            crate::warn!("no backend to handle client {client_id:x}");
                        }
                        Some(backend) => {
                            let (from_rdp_send, from_rdp_recv) =
                                crossbeam_channel::bounded(CLIENT_CHUNK_BUFFER_SIZE);
                            ve.insert(from_rdp_send);

                            let stream =
                                rdp::RdpStream::new(self, service, client_id, from_rdp_recv);
                            stream.accept()?;

                            thread::Builder::new()
                                .name(format!(
                                    "{} {service} {client_id:x}",
                                    service::Kind::Backend
                                ))
                                .spawn_scoped(scope, move || {
                                    if let Err(e) = (backend.handler)(stream) {
                                        crate::debug!("error: {e}");
                                    }
                                })
                                .unwrap();
                        }
                    }
                }
            },
        }

        Ok(())
    }

    pub fn start(
        &self,
        service_kind: service::Kind,
        from_rdp: &crossbeam_channel::Receiver<api::ChannelControl>,
    ) -> Result<(), api::Error> {
        thread::scope(|scope| {
            loop {
                let control_chunk = from_rdp.recv()?;

                match control_chunk {
                    api::ChannelControl::Shutdown => {
                        self.shutdown();
                    }
                    api::ChannelControl::ResetClient => {
                        crate::error!("discarding reset client request");
                    }
                    api::ChannelControl::SendInputSetting(_) => {
                        crate::error!("discarding input setting request");
                    }
                    api::ChannelControl::SendInputAction(_) => {
                        crate::error!("discarding input action request");
                    }
                    api::ChannelControl::SendChunk(chunk) => match chunk.chunk_type() {
                        Err(_) => {
                            crate::error!("discarding invalid chunk");
                        }
                        Ok(chunk_type) => {
                            let client_id = chunk.client_id();

                            match chunk_type {
                                api::ChunkType::Start => match service_kind {
                                    #[cfg(feature = "frontend")]
                                    service::Kind::Frontend => {
                                        let _ = scope;
                                        unimplemented!("accept connections");
                                    }
                                    #[cfg(feature = "backend")]
                                    service::Kind::Backend => {
                                        let payload = chunk.payload();
                                        self.handle_backend_start(client_id, payload, scope)?;
                                    }
                                },
                                api::ChunkType::Data => {
                                    if let Some(client) = self
                                        .clients
                                        .read()
                                        .map_err(|e| {
                                            io::Error::new(io::ErrorKind::BrokenPipe, e.to_string())
                                        })?
                                        .get(&client_id)
                                    {
                                        if client.send(chunk).is_err() {
                                            crate::warn!(
                                                "error sending to disconnected client {client_id:x}"
                                            );
                                        }
                                    } else {
                                        crate::debug!(
                                            "discarding chunk for unknown client {client_id:x}"
                                        );
                                        let _ = self.send_chunk(api::Chunk::end(client_id));
                                    }
                                }
                                api::ChunkType::End => {
                                    let value = self
                                        .clients
                                        .write()
                                        .map_err(|e| {
                                            io::Error::new(io::ErrorKind::BrokenPipe, e.to_string())
                                        })?
                                        .remove(&client_id);
                                    if let Some(client) = value {
                                        if client.send(chunk).is_err() {
                                            crate::warn!(
                                                "error sending to disconnected client {client_id:x}"
                                            );
                                        }
                                    } else {
                                        crate::debug!(
                                            "discarding chunk for unknown client {client_id:x}"
                                        );
                                    }
                                }
                            }
                        }
                    },
                }
            }
        })
    }
}

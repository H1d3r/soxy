use crate::service;

#[cfg(feature = "frontend")]
mod frontend;

pub(crate) static SERVICE: service::Service = service::Service {
    name: "stage0",
    #[cfg(feature = "frontend")]
    frontend: Some(service::Frontend::Tcp {
        default_port: 1082,
        handler: frontend::tcp_handler,
    }),
    #[cfg(feature = "backend")]
    backend: None,
};

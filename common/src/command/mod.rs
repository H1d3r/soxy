use crate::service;

#[cfg(feature = "backend")]
mod backend;
#[cfg(feature = "frontend")]
mod frontend;

pub(crate) static SERVICE: service::Service = service::Service {
    name: "command",
    #[cfg(feature = "frontend")]
    frontend: Some(service::Frontend::Tcp {
        default_port: 3031,
        handler: frontend::tcp_frontend_handler,
    }),
    #[cfg(feature = "backend")]
    backend: Some(service::Backend {
        handler: backend::backend_handler,
    }),
};

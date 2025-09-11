#[cfg(not(any(feature = "dvc", feature = "svc")))]
compile_error! {"feature \"dvc\" and/or feature \"svc\" must be enabled"}

use std::{ffi, fmt, ops, os};
use windows_sys as ws;

#[cfg(feature = "dvc")]
mod dvc;
#[cfg(feature = "svc")]
mod svc;

#[cfg(feature = "svc")]
type VirtualChannelOpen = unsafe extern "system" fn(
    hserver: ws::Win32::Foundation::HANDLE,
    sessionid: os::raw::c_uint,
    pvirtualname: *mut os::raw::c_char,
) -> ws::Win32::Foundation::HANDLE;

#[cfg(feature = "dvc")]
type VirtualChannelOpenEx = unsafe extern "system" fn(
    sessionid: os::raw::c_uint,
    pvirtualname: *mut os::raw::c_char,
    flags: os::raw::c_uint,
) -> ws::Win32::Foundation::HANDLE;

type VirtualChannelQuery = unsafe extern "system" fn(
    hchannelhandle: ws::Win32::Foundation::HANDLE,
    wtsvirtualclass: ws::Win32::System::RemoteDesktop::WTS_VIRTUAL_CLASS,
    ppbuffer: *mut *mut os::raw::c_void,
    pbytesreturned: *mut os::raw::c_ulong,
) -> ws::core::BOOL;

type VirtualChannelRead = unsafe extern "system" fn(
    hchannelhandle: ws::Win32::Foundation::HANDLE,
    timeout: os::raw::c_ulong,
    buffer: *mut os::raw::c_uchar,
    buffersize: os::raw::c_ulong,
    pbytesread: *mut os::raw::c_ulong,
) -> ws::core::BOOL;

type VirtualChannelWrite = unsafe extern "system" fn(
    hchannelhandle: ws::Win32::Foundation::HANDLE,
    buffer: *const os::raw::c_uchar,
    length: os::raw::c_ulong,
    pbyteswritten: *mut os::raw::c_ulong,
) -> ws::core::BOOL;

type VirtualChannelClose =
    unsafe extern "system" fn(hchannelhandle: ws::Win32::Foundation::HANDLE) -> ws::core::BOOL;

pub(crate) enum Error {
    NoLibraryFound,
    LibraryLoading(libloading::Error),
    #[cfg(target_os = "windows")]
    WsaStartupFailed(i32),
    OpenChannelFailed(String),
    CloseChannelFailed(String),
    ReadFailed(String),
    WriteFailed(String),
    #[cfg(target_os = "windows")]
    QueryFailed(String),
    #[cfg(target_os = "windows")]
    DuplicateHandleFailed(String),
    #[cfg(target_os = "windows")]
    CreateEventFailed(String),
}

impl From<libloading::Error> for Error {
    fn from(e: libloading::Error) -> Self {
        Self::LibraryLoading(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Self::NoLibraryFound => write!(f, "no library found"),
            Self::LibraryLoading(e) => write!(f, "library loading error: {e}"),
            #[cfg(target_os = "windows")]
            Self::WsaStartupFailed(e) => write!(f, "WSAStartup failed with error code {e}"),
            Self::OpenChannelFailed(err) => {
                write!(f, "open failed (last_error = {err})")
            }
            Self::CloseChannelFailed(err) => {
                write!(f, "close failed (last_error = {err})")
            }
            Self::ReadFailed(err) => {
                write!(f, "read failed (last error = {err})")
            }
            Self::WriteFailed(err) => {
                write!(f, "write failed (last error = {err})")
            }
            #[cfg(target_os = "windows")]
            Self::QueryFailed(err) => {
                write!(f, "query failed (last error = {err})")
            }
            #[cfg(target_os = "windows")]
            Self::DuplicateHandleFailed(err) => {
                write!(f, "duplicate handle failed (last error = {err})")
            }
            #[cfg(target_os = "windows")]
            Self::CreateEventFailed(err) => {
                write!(f, "create event failed (last error = {err})")
            }
        }
    }
}

pub(crate) struct Libraries {
    #[cfg(all(feature = "svc", target_os = "windows"))]
    vdp_rdpvcbridge: Option<libloading::Library>,
    #[cfg(feature = "svc")]
    citrix: Option<libloading::Library>,
    #[cfg(target_os = "windows")]
    wtsapi32: Option<libloading::Library>,
    xrdpapi: Option<libloading::Library>,
}

impl Libraries {
    #[cfg(all(feature = "svc", target_os = "windows"))]
    pub(crate) fn horizon(&self) -> Option<&libloading::Library> {
        self.vdp_rdpvcbridge.as_ref()
    }

    #[cfg(feature = "svc")]
    pub(crate) fn citrix(&self) -> Option<&libloading::Library> {
        self.citrix.as_ref()
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn wts(&self) -> Option<&libloading::Library> {
        self.wtsapi32.as_ref()
    }

    pub(crate) fn xrdp(&self) -> Option<&libloading::Library> {
        self.xrdpapi.as_ref()
    }

    pub(crate) fn load() -> Self {
        unsafe {
            common::trace!("trying to load Citrix library");
            #[cfg(target_os = "windows")]
            let citrix = libloading::Library::new(libloading::library_filename("wfapi64")).ok();
            #[cfg(not(target_os = "windows"))]
            let citrix = libloading::Library::new(libloading::library_filename("winsta")).ok();

            common::trace!("trying to load Horizon library");
            let vdp_rdpvcbridge =
                libloading::Library::new(libloading::library_filename("vdp_rdpvcbridge")).ok();

            common::trace!("trying to load XRDP library");
            let xrdpapi = libloading::Library::new(libloading::library_filename("xrdpapi")).ok();

            #[cfg(target_os = "windows")]
            let wtsapi32 = {
                common::trace!("trying to load WTS library");
                libloading::Library::new(libloading::library_filename("wtsapi32")).ok()
            };

            let found: Vec<&str> = vec![
                citrix.as_ref().map(|_| "Citrix"),
                vdp_rdpvcbridge.as_ref().map(|_| "Horizon"),
                xrdpapi.as_ref().map(|_| "XRDP"),
                #[cfg(target_os = "windows")]
                wtsapi32.as_ref().map(|_| "WTS"),
            ]
            .into_iter()
            .flatten()
            .collect();

            common::info!("found libraries: {}", found.join(", "));

            Self {
                #[cfg(all(feature = "svc", target_os = "windows"))]
                vdp_rdpvcbridge,
                #[cfg(feature = "svc")]
                citrix,
                #[cfg(target_os = "windows")]
                wtsapi32,
                xrdpapi,
            }
        }
    }
}

pub(crate) trait VirtualChannel<'a>: Sized + Send + Sync {
    type Handle: Handle;
    fn load(libs: &'a Libraries) -> Result<Self, Error>;
    fn open(&self, name: [ffi::c_char; 8]) -> Result<Self::Handle, Error>;
}

pub(crate) struct GenericChannel<'a> {
    #[cfg(feature = "dvc")]
    dvc: Option<dvc::Dvc<'a>>,
    #[cfg(feature = "svc")]
    svc: Option<svc::Svc<'a>>,
}

impl<'a> VirtualChannel<'a> for GenericChannel<'a> {
    type Handle = GenericHandle<'a>;

    fn load(libs: &'a Libraries) -> Result<Self, Error> {
        #[cfg(not(feature = "dvc"))]
        let dvc: Option<()> = None;
        #[cfg(feature = "dvc")]
        let dvc = dvc::Dvc::load(libs)
            .inspect_err(|e| {
                common::warn!("no DVC loaded: {e}");
            })
            .ok();

        #[cfg(not(feature = "svc"))]
        let svc: Option<()> = None;
        #[cfg(feature = "svc")]
        let svc = svc::Svc::load(libs)
            .inspect_err(|e| {
                common::warn!("no SVC loaded: {e}");
            })
            .ok();

        if svc.is_none() && dvc.is_none() {
            return Err(Error::NoLibraryFound);
        }

        Ok(Self {
            #[cfg(feature = "dvc")]
            dvc,
            #[cfg(feature = "svc")]
            svc,
        })
    }

    fn open(&self, name: [ffi::c_char; 8]) -> Result<Self::Handle, Error> {
        #[cfg(not(feature = "svc"))]
        let svc: Result<Self::Handle, _> = Err(Error::NoLibraryFound);
        #[cfg(feature = "svc")]
        let svc = self
            .svc
            .as_ref()
            .ok_or(Error::NoLibraryFound)
            .and_then(|svc| svc.open(name))
            .map(GenericHandle::Static)
            .inspect_err(|e| common::error!("failed to open SVC: {e}"));

        #[cfg(not(feature = "dvc"))]
        let dvc: Result<Self::Handle, _> = Err(Error::NoLibraryFound);
        #[cfg(feature = "dvc")]
        let dvc = self
            .dvc
            .as_ref()
            .ok_or(Error::NoLibraryFound)
            .and_then(|dvc| dvc.open(name))
            .map(GenericHandle::Dynamic)
            .inspect_err(|e| common::error!("failed to open DVC: {e}"));

        dvc.or(svc)
    }
}

pub(crate) trait Handle: Send + Sync {
    fn display_name(&self) -> &str;
    fn read(&self, data: &mut [u8]) -> Result<ops::Range<usize>, Error>;
    fn write(&self, data: &[u8]) -> Result<usize, Error>;
    fn close(self) -> Result<(), Error>;
}

pub(crate) enum GenericHandle<'a> {
    #[cfg(feature = "dvc")]
    Dynamic(dvc::Handle<'a>),
    #[cfg(feature = "svc")]
    Static(svc::Handle<'a>),
}

impl Handle for GenericHandle<'_> {
    fn display_name(&self) -> &str {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvch) => dvch.display_name(),
            #[cfg(feature = "svc")]
            Self::Static(svch) => svch.display_name(),
        }
    }

    fn read(&self, data: &mut [u8]) -> Result<ops::Range<usize>, Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvch) => dvch.read(data),
            #[cfg(feature = "svc")]
            Self::Static(svch) => svch.read(data),
        }
    }

    fn write(&self, data: &[u8]) -> Result<usize, Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvch) => dvch.write(data),
            #[cfg(feature = "svc")]
            Self::Static(svch) => svch.write(data),
        }
    }

    fn close(self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvch) => dvch.close(),
            #[cfg(feature = "svc")]
            Self::Static(svch) => svch.close(),
        }
    }
}

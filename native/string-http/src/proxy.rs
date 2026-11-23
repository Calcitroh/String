use regex_lite::Regex;
use std::{io, net::IpAddr};
use ureq::Proxy;
use url::Url;
use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
    },
    core::{PCWSTR, w},
};

const INTERNET: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");

#[derive(Default)]
pub(super) struct ProxySettings {
    http: Option<Route>,
    https: Option<Route>,
    socks: Option<Route>,
    local: bool,
    bypass: Vec<Regex>,
}

#[derive(Clone)]
enum Route {
    Proxy(Proxy),
    UnsupportedSocks,
    Invalid,
}

impl Route {
    fn from_proxy(proxy: Proxy) -> Self {
        match proxy.protocol() {
            ureq::ProxyProtocol::Http | ureq::ProxyProtocol::Https => Self::Proxy(proxy),
            _ => Self::UnsupportedSocks,
        }
    }
}

impl ProxySettings {
    pub(super) fn system() -> Self {
        let mut enabled = 0u32;
        let mut size = size_of_val(&enabled) as u32;
        // HKCU is predefined. The result buffers are writable for the byte counts supplied.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                INTERNET,
                w!("ProxyEnable"),
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut enabled).cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS || enabled == 0 {
            // Keep the environment proxy used by earlier builds when Windows has no manual proxy.
            let proxy = Proxy::try_from_env().map(Route::from_proxy);
            return Self {
                http: proxy.clone(),
                https: proxy,
                ..Default::default()
            };
        }
        Self::parse(
            true,
            &registry_string(w!("ProxyServer")),
            &registry_string(w!("ProxyOverride")),
        )
    }

    pub(super) fn parse(enabled: bool, server: &str, exceptions: &str) -> Self {
        let mut settings = Self::default();
        if !enabled {
            return settings;
        }
        for entry in server.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (scheme, address) = entry
                .split_once('=')
                .map_or(("", entry), |(s, a)| (s.trim(), a.trim()));
            let proxy = if address.contains("://") {
                Proxy::new(address)
            } else {
                Proxy::new(&format!("http://{address}"))
            };
            let route = Some(proxy.map(Route::from_proxy).unwrap_or(Route::Invalid));
            match scheme.to_ascii_lowercase().as_str() {
                "" => {
                    settings.http = route;
                    settings.https = settings.http.clone();
                }
                "http" => settings.http = route,
                "https" => settings.https = route,
                "socks" => settings.socks = Some(Route::UnsupportedSocks),
                _ => {}
            }
        }
        for pattern in exceptions
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if pattern.eq_ignore_ascii_case("<local>") {
                settings.local = true;
            } else {
                // Windows exceptions may qualify a host with a scheme or port.
                let (scheme, host_port) = pattern
                    .split_once("://")
                    .map_or(("[a-z]+".into(), pattern), |(s, h)| {
                        (regex_lite::escape(s), h)
                    });
                let glob = host_port
                    .split('*')
                    .map(regex_lite::escape)
                    .collect::<Vec<_>>()
                    .join(".*");
                let has_port = host_port.rsplit(']').next().unwrap_or("").contains(':');
                let port = if has_port { "" } else { ":[0-9]+" };
                if let Ok(re) = Regex::new(&format!("(?i)^{scheme}://{glob}{port}$")) {
                    settings.bypass.push(re);
                }
            }
        }
        settings
    }

    pub(super) fn route(&self, url: &Url) -> io::Result<Option<&Proxy>> {
        let route = match url.scheme() {
            "http" => self.http.as_ref().or(self.socks.as_ref()),
            "https" => self.https.as_ref().or(self.socks.as_ref()),
            _ => None,
        };
        let (Some(route), Some(host)) = (route, url.host_str()) else {
            return Ok(None);
        };
        let ip = host.trim_matches(['[', ']']).parse::<IpAddr>().ok();
        if ip.is_some_and(|ip| ip.is_loopback())
            || host.eq_ignore_ascii_case("localhost")
            || (self.local && ip.is_none() && !host.contains('.'))
        {
            return Ok(None);
        }
        if !self.bypass.is_empty() {
            let origin = format!(
                "{}://{host}:{}",
                url.scheme(),
                url.port_or_known_default().unwrap()
            );
            if self.bypass.iter().any(|re| re.is_match(&origin)) {
                return Ok(None);
            }
        }
        match route {
            Route::Proxy(proxy) => Ok(Some(proxy)),
            Route::UnsupportedSocks => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SOCKS proxies are unsupported in this build",
            )),
            Route::Invalid => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid Windows proxy address",
            )),
        }
    }
}

fn registry_string(name: PCWSTR) -> String {
    let mut bytes = 0;
    let read = |buffer, size: &mut u32| unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            INTERNET,
            name,
            RRF_RT_REG_SZ,
            None,
            buffer,
            Some(size),
        )
    };
    if read(None, &mut bytes) != ERROR_SUCCESS {
        return String::new();
    }
    let mut value = vec![0u16; (bytes as usize).div_ceil(2)];
    if read(Some(value.as_mut_ptr().cast()), &mut bytes) != ERROR_SUCCESS {
        return String::new();
    }
    String::from_utf16_lossy(&value[..value.iter().position(|&v| v == 0).unwrap_or(value.len())])
}

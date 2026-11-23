//! One connection pool for the release feed, online lyrics and cover searches.
mod proxy;
use proxy::ProxySettings;
use std::{
    io,
    sync::LazyLock,
    time::{Duration, Instant},
};
use ureq::{
    Agent, Body, Error, RequestBuilder,
    http::Response,
    tls::{RootCerts, TlsConfig},
    typestate::WithoutBody,
};
use url::Url;

pub static HTTP: LazyLock<Client> = LazyLock::new(|| Client::new(ProxySettings::system()));

pub struct Client {
    agent: Agent,
    proxy: ProxySettings,
}
impl Client {
    fn new(proxy: ProxySettings) -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .proxy(None)
            .max_redirects(0)
            .build();
        Self {
            agent: config.into(),
            proxy,
        }
    }

    /// Configure each GET with the caller's own headers and timeouts. Redirects
    /// share the original deadline and use the proxy for their destination.
    pub fn get(
        &self,
        address: &str,
        timeout: Duration,
        redirects: u32,
        configure: impl Fn(RequestBuilder<WithoutBody>) -> RequestBuilder<WithoutBody>,
    ) -> Result<Response<Body>, Error> {
        let started = Instant::now();
        let mut url = Url::parse(address).map_err(io::Error::other)?;
        let mut redirects_left = redirects;
        loop {
            url.set_fragment(None);
            let remaining = timeout
                .checked_sub(started.elapsed())
                .ok_or(Error::Timeout(ureq::Timeout::Global))?;
            let proxy = self.proxy.route(&url)?.cloned();
            let request = configure(self.agent.get(url.as_str()));
            let response = request
                .config()
                .proxy(proxy)
                .timeout_global(Some(remaining))
                .build()
                .call()?;
            if redirects == 0 || !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308)
            {
                return Ok(response);
            }
            let Some(location) = response.headers().get("location") else {
                return Ok(response);
            };
            if redirects_left == 0 {
                return Err(Error::TooManyRedirects);
            }
            redirects_left -= 1;
            url = url
                .join(location.to_str().map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        }
    }
}

#[cfg(test)]
mod tests;

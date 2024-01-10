//! usbsas's uploader, downloader and analyzer processes.

pub mod analyzer;
pub mod downloader;
pub mod uploader;

pub use analyzer::Analyzer;
pub use downloader::Downloader;
pub use uploader::Uploader;

use base64::{engine as b64eng, Engine as _};
#[cfg(feature = "authkrb")]
use libgssapi::{
    context::{ClientCtx, CtxFlags, SecurityContext},
    credential::{Cred, CredUsage},
    name::Name,
    oid::{OidSet, GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE},
};
use reqwest::{
    blocking::{Body, Client, Response},
    header::{HeaderMap, HeaderValue},
    Method, StatusCode,
};
use std::time::Duration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("reqwest error: {0}")]
    ReqwestHeader(#[from] reqwest::header::InvalidHeaderValue),
    #[error("reqwest error: {0}")]
    ReqwestStr(#[from] reqwest::header::ToStrError),
    #[cfg(feature = "authkrb")]
    #[error("gssapi error: {0}")]
    Gssapi(#[from] libgssapi::error::Error),
    #[cfg(feature = "authkrb")]
    #[error("Negotiation error")]
    Nego,
    #[cfg(feature = "authkrb")]
    #[error("base64 error: {0}")]
    B64(#[from] base64::DecodeError),
    #[error("{0}")]
    Error(String),
    #[error("sandbox: {0}")]
    Sandbox(#[from] usbsas_sandbox::Error),
    #[error("json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("Bad Request")]
    BadRequest,
    #[error("No conf")]
    NoConf,
    #[error("Remote server error")]
    BadResponse,
    #[error("Bad Response")]
    Remote,
    #[error("State error")]
    State,
    #[error("{0}")]
    Upload(String),
}
pub type Result<T> = std::result::Result<T, Error>;

// Wrapper around reqwest::Client to transparently perform kerberos authentication
pub(crate) struct HttpClient {
    client: Client,
    headers: HeaderMap,
    #[cfg(feature = "authkrb")]
    krb_service_name: Option<String>,
}

impl HttpClient {
    fn new(#[cfg(feature = "authkrb")] krb_service_name: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(None)
            .gzip(true)
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            headers: HeaderMap::new(),
            #[cfg(feature = "authkrb")]
            krb_service_name,
        })
    }

    #[cfg(feature = "authkrb")]
    fn req_with_krb_auth(&mut self, method: reqwest::Method, url: &str) -> Result<Response> {
        let mut resp_ret: Option<Response> = None;
        if let Some(krb_service_name) = &self.krb_service_name {
            let desired_mechs = {
                let mut set = OidSet::new()?;
                set.add(&GSS_MECH_KRB5)?;
                set
            };
            let service = Name::new(krb_service_name.as_bytes(), Some(&GSS_NT_HOSTBASED_SERVICE))?;
            let client_cred = Cred::acquire(None, None, CredUsage::Initiate, Some(&desired_mechs))?;
            let mut client_ctx = ClientCtx::new(
                Some(client_cred),
                service,
                CtxFlags::GSS_C_MUTUAL_FLAG,
                Some(&GSS_MECH_KRB5),
            );

            let mut server_token: Option<Vec<u8>> = None;
            // Mutually authenticate with server
            loop {
                match client_ctx.step(server_token.as_deref(), None) {
                    Ok(None) => {
                        let flags = client_ctx.flags()?;
                        let required_flags = CtxFlags::GSS_C_MUTUAL_FLAG
                            | CtxFlags::GSS_C_CONF_FLAG
                            | CtxFlags::GSS_C_INTEG_FLAG;
                        if flags & required_flags != required_flags {
                            return Err(Error::Nego);
                        };
                        log::debug!("Kerberos authentication complete");
                        break;
                    }
                    Ok(Some(client_token)) => {
                        self.headers.insert(
                            reqwest::header::AUTHORIZATION,
                            format!(
                                "Negotiate {}",
                                &b64eng::general_purpose::STANDARD
                                    .encode::<&[u8]>(client_token.as_ref())
                            )
                            .parse()?,
                        );
                        let resp = self
                            .client
                            .request(method.clone(), url)
                            .headers(self.headers.clone())
                            .send()?;
                        if !resp.status().is_success() {
                            return Err(Error::Nego);
                        }
                        let authenticate_h =
                            resp.headers().get("www-authenticate").ok_or(Error::Nego)?;
                        server_token = Some(
                            b64eng::general_purpose::STANDARD
                                .decode(&authenticate_h.to_str()?[10..])?,
                        );
                        resp_ret = Some(resp);
                    }
                    Err(err) => {
                        log::error!("{}", err);
                        return Err(Error::Nego);
                    }
                }
            }
        }
        if let Some(resp) = resp_ret {
            Ok(resp)
        } else {
            Err(Error::Nego)
        }
    }

    fn get(&mut self, url: &str) -> Result<Response> {
        self.headers
            .insert(reqwest::header::REFERER, HeaderValue::from_str(url)?);
        let mut resp = self.client.get(url).headers(self.headers.clone()).send()?;
        #[cfg(feature = "authkrb")]
        if resp.status() == StatusCode::UNAUTHORIZED && self.krb_service_name.is_some() {
            resp = self.req_with_krb_auth(Method::GET, url)?;
        }
        Ok(resp)
    }

    fn head(&mut self, url: &str) -> Result<Response> {
        self.headers
            .insert(reqwest::header::REFERER, HeaderValue::from_str(url)?);
        let mut resp = self.client.head(url).headers(self.headers.clone()).send()?;
        #[cfg(feature = "authkrb")]
        if resp.status() == StatusCode::UNAUTHORIZED && self.krb_service_name.is_some() {
            resp = self.req_with_krb_auth(Method::HEAD, url)?;
        }
        Ok(resp)
    }

    fn post(&mut self, url: &str, body: Body) -> Result<Response> {
        self.headers
            .insert(reqwest::header::REFERER, HeaderValue::from_str(url)?);
        // First try a OPTIONS on url to avoid uploading (potentially large) body
        // while unauthenticated (preflight request)
        #[cfg(feature = "authkrb")]
        if self.krb_service_name.is_some() {
            let resp = self
                .client
                .request(Method::OPTIONS, url)
                .headers(self.headers.clone())
                .send()?;
            if resp.status() == StatusCode::UNAUTHORIZED {
                self.req_with_krb_auth(Method::OPTIONS, url)?;
            }
        }
        Ok(self
            .client
            .post(url)
            .headers(self.headers.clone())
            .body(body)
            .send()?)
    }
}

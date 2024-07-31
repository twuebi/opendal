// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use http::Response;
use http::StatusCode;
use log::debug;
use reqsign::AzureStorageConfig;
use reqsign::AzureStorageLoader;
use reqsign::AzureStorageSigner;
use serde::Deserialize;
use serde::Serialize;

use super::core::AzdlsCore;
use super::error::parse_error;
use super::lister::AzdlsLister;
use super::writer::AzdlsWriter;
use super::writer::AzdlsWriters;
use crate::raw::*;
use crate::*;

/// Known endpoint suffix Azure Data Lake Storage Gen2 URI syntax.
/// Azure public cloud: https://accountname.dfs.core.windows.net
/// Azure US Government: https://accountname.dfs.core.usgovcloudapi.net
/// Azure China: https://accountname.dfs.core.chinacloudapi.cn
const KNOWN_AZDLS_ENDPOINT_SUFFIX: &[&str] = &[
    "dfs.core.windows.net",
    "dfs.core.usgovcloudapi.net",
    "dfs.core.chinacloudapi.cn",
];

/// Azure Data Lake Storage Gen2 Support.
#[derive(Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AzdlsConfig {
    /// Root of this backend.
    pub root: Option<String>,
    /// Filesystem name of this backend.
    pub filesystem: String,
    /// Endpoint of this backend.
    pub endpoint: Option<String>,
    /// Account name of this backend.
    pub account_name: Option<String>,
    /// Account key of this backend.
    /// - required for shared_key authentication
    pub account_key: Option<String>,
    /// client_secret
    /// The client secret of the service principal.
    /// - required for client_credentials authentication
    pub client_secret: Option<String>,
    /// tenant_id
    /// The tenant id of the service principal.
    /// - required for client_credentials authentication
    pub tenant_id: Option<String>,
    /// client_id
    /// The client id of the service principal.
    /// - required for client_credentials authentication
    pub client_id: Option<String>,
    /// authority_host
    /// The authority host of the service principal.
    /// - required for client_credentials authentication
    /// - default value: `https://login.microsoftonline.com`
    pub authority_host: Option<String>,
}

impl Debug for AzdlsConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("AzdlsConfig");

        ds.field("root", &self.root);
        ds.field("filesystem", &self.filesystem);
        ds.field("endpoint", &self.endpoint);

        if self.account_name.is_some() {
            ds.field("account_name", &"<redacted>");
        }
        if self.account_key.is_some() {
            ds.field("account_key", &"<redacted>");
        }

        if self.client_secret.is_some() {
            ds.field("client_secret", &"<redacted>");
        }

        if self.tenant_id.is_some() {
            ds.field("tenant_id", &self.tenant_id);
        }

        if self.client_id.is_some() {
            ds.field("client_id", &self.client_id);
        }

        ds.finish()
    }
}

impl Configurator for AzdlsConfig {
    fn into_builder(self) -> impl Builder {
        AzdlsBuilder {
            config: self,
            http_client: None,
        }
    }
}

/// Azure Data Lake Storage Gen2 Support.
#[doc = include_str!("docs.md")]
#[derive(Default, Clone)]
pub struct AzdlsBuilder {
    config: AzdlsConfig,
    http_client: Option<HttpClient>,
}

impl Debug for AzdlsBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("AzdlsBuilder");

        ds.field("config", &self.config);

        ds.finish()
    }
}

impl AzdlsBuilder {
    /// Set root of this backend.
    ///
    /// All operations will happen under this root.
    pub fn root(mut self, root: &str) -> Self {
        if !root.is_empty() {
            self.config.root = Some(root.to_string())
        }

        self
    }

    /// Set filesystem name of this backend.
    pub fn filesystem(mut self, filesystem: &str) -> Self {
        self.config.filesystem = filesystem.to_string();

        self
    }

    /// Set endpoint of this backend.
    ///
    /// Endpoint must be full uri, e.g.
    ///
    /// - Azblob: `https://accountname.blob.core.windows.net`
    /// - Azurite: `http://127.0.0.1:10000/devstoreaccount1`
    pub fn endpoint(mut self, endpoint: &str) -> Self {
        if !endpoint.is_empty() {
            // Trim trailing `/` so that we can accept `http://127.0.0.1:9000/`
            self.config.endpoint = Some(endpoint.trim_end_matches('/').to_string());
        }

        self
    }

    /// Set account_name of this backend.
    ///
    /// - If account_name is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    pub fn account_name(mut self, account_name: &str) -> Self {
        if !account_name.is_empty() {
            self.config.account_name = Some(account_name.to_string());
        }

        self
    }

    /// Set account_key of this backend.
    ///
    /// - If account_key is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    pub fn account_key(mut self, account_key: &str) -> Self {
        if !account_key.is_empty() {
            self.config.account_key = Some(account_key.to_string());
        }

        self
    }

    /// Set client_secret of this backend.
    ///
    /// - If client_secret is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    /// - required for client_credentials authentication
    pub fn client_secret(mut self, client_secret: &str) -> Self {
        if !client_secret.is_empty() {
            self.config.client_secret = Some(client_secret.to_string());
        }

        self
    }

    /// Set tenant_id of this backend.
    ///
    /// - If tenant_id is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    /// - required for client_credentials authentication
    pub fn tenant_id(mut self, tenant_id: &str) -> Self {
        if !tenant_id.is_empty() {
            self.config.tenant_id = Some(tenant_id.to_string());
        }

        self
    }

    /// Set client_id of this backend.
    ///
    /// - If client_id is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    /// - required for client_credentials authentication
    pub fn client_id(mut self, client_id: &str) -> Self {
        if !client_id.is_empty() {
            self.config.client_id = Some(client_id.to_string());
        }

        self
    }

    /// Set authority_host of this backend.
    ///
    /// - If authority_host is set, we will take user's input first.
    /// - If not, we will try to load it from environment.
    /// - default value: `https://login.microsoftonline.com`
    pub fn authority_host(mut self, authority_host: &str) -> Self {
        if !authority_host.is_empty() {
            self.config.authority_host = Some(authority_host.to_string());
        }

        self
    }

    /// Specify the http client that used by this service.
    ///
    /// # Notes
    ///
    /// This API is part of OpenDAL's Raw API. `HttpClient` could be changed
    /// during minor updates.
    pub fn http_client(mut self, client: HttpClient) -> Self {
        self.http_client = Some(client);
        self
    }
}

impl Builder for AzdlsBuilder {
    const SCHEME: Scheme = Scheme::Azdls;
    type Config = AzdlsConfig;

    fn build(self) -> Result<impl Access> {
        debug!("backend build started: {:?}", &self);

        let root = normalize_root(&self.config.root.unwrap_or_default());
        debug!("backend use root {}", root);

        // Handle endpoint, region and container name.
        let filesystem = match self.config.filesystem.is_empty() {
            false => Ok(&self.config.filesystem),
            true => Err(Error::new(ErrorKind::ConfigInvalid, "filesystem is empty")
                .with_operation("Builder::build")
                .with_context("service", Scheme::Azdls)),
        }?;
        debug!("backend use filesystem {}", &filesystem);

        let endpoint = match &self.config.endpoint {
            Some(endpoint) => Ok(endpoint.clone().trim_end_matches('/').to_string()),
            None => Err(Error::new(ErrorKind::ConfigInvalid, "endpoint is empty")
                .with_operation("Builder::build")
                .with_context("service", Scheme::Azdls)),
        }?;
        debug!("backend use endpoint {}", &endpoint);

        let client = if let Some(client) = self.http_client {
            client
        } else {
            HttpClient::new().map_err(|err| {
                err.with_operation("Builder::build")
                    .with_context("service", Scheme::Azdls)
            })?
        };

        let config_loader = AzureStorageConfig {
            account_name: self
                .config
                .account_name
                .clone()
                .or_else(|| infer_storage_name_from_endpoint(endpoint.as_str())),
            account_key: self.config.account_key.clone(),
            sas_token: None,
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            tenant_id: self.config.tenant_id.clone(),
            authority_host: Some(self.config.authority_host.clone().unwrap_or_else(|| {
                "https://login.microsoftonline.com".to_string()
            })),
            ..Default::default()
        };

        let cred_loader = AzureStorageLoader::new(config_loader);
        let signer = AzureStorageSigner::new();
        Ok(AzdlsBackend {
            core: Arc::new(AzdlsCore {
                filesystem: self.config.filesystem.clone(),
                root,
                endpoint,
                client,
                loader: cred_loader,
                signer,
            }),
        })
    }
}

/// Backend for azblob services.
#[derive(Debug, Clone)]
pub struct AzdlsBackend {
    core: Arc<AzdlsCore>,
}

impl Access for AzdlsBackend {
    type Reader = HttpBody;
    type Writer = AzdlsWriters;
    type Lister = oio::PageLister<AzdlsLister>;
    type BlockingReader = ();
    type BlockingWriter = ();
    type BlockingLister = ();

    fn info(&self) -> Arc<AccessorInfo> {
        let mut am = AccessorInfo::default();
        am.set_scheme(Scheme::Azdls)
            .set_root(&self.core.root)
            .set_name(&self.core.filesystem)
            .set_native_capability(Capability {
                stat: true,

                read: true,

                write: true,
                write_can_append: true,
                create_dir: true,
                delete: true,
                rename: true,

                list: true,

                ..Default::default()
            });

        am.into()
    }

    async fn create_dir(&self, path: &str, _: OpCreateDir) -> Result<RpCreateDir> {
        let mut req = self.core.azdls_create_request(
            path,
            "directory",
            &OpWrite::default(),
            Buffer::new(),
        )?;

        self.core.sign(&mut req).await?;

        let resp = self.core.send(req).await?;

        let status = resp.status();

        match status {
            StatusCode::CREATED | StatusCode::OK => Ok(RpCreateDir::default()),
            _ => Err(parse_error(resp).await?),
        }
    }

    async fn stat(&self, path: &str, _: OpStat) -> Result<RpStat> {
        // Stat root always returns a DIR.
        if path == "/" {
            return Ok(RpStat::new(Metadata::new(EntryMode::DIR)));
        }

        let resp = self.core.azdls_get_properties(path).await?;

        if resp.status() != StatusCode::OK {
            return Err(parse_error(resp).await?);
        }

        let mut meta = parse_into_metadata(path, resp.headers())?;
        let resource = resp
            .headers()
            .get("x-ms-resource-type")
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Unexpected,
                    "azdls should return x-ms-resource-type header, but it's missing",
                )
            })?
            .to_str()
            .map_err(|err| {
                Error::new(
                    ErrorKind::Unexpected,
                    "azdls should return x-ms-resource-type header, but it's not a valid string",
                )
                .set_source(err)
            })?;

        meta = match resource {
            "file" => meta.with_mode(EntryMode::FILE),
            "directory" => meta.with_mode(EntryMode::DIR),
            v => {
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    "azdls returns not supported x-ms-resource-type",
                )
                .with_context("resource", v))
            }
        };

        Ok(RpStat::new(meta))
    }

    async fn read(&self, path: &str, args: OpRead) -> Result<(RpRead, Self::Reader)> {
        let resp = self.core.azdls_read(path, args.range()).await?;

        let status = resp.status();
        match status {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => Ok((RpRead::new(), resp.into_body())),
            _ => {
                let (part, mut body) = resp.into_parts();
                let buf = body.to_buffer().await?;
                Err(parse_error(Response::from_parts(part, buf)).await?)
            }
        }
    }

    async fn write(&self, path: &str, args: OpWrite) -> Result<(RpWrite, Self::Writer)> {
        let w = AzdlsWriter::new(self.core.clone(), args.clone(), path.to_string());
        let w = if args.append() {
            AzdlsWriters::Two(oio::AppendWriter::new(w))
        } else {
            AzdlsWriters::One(oio::OneShotWriter::new(w))
        };
        Ok((RpWrite::default(), w))
    }

    async fn delete(&self, path: &str, _: OpDelete) -> Result<RpDelete> {
        let resp = self.core.azdls_delete(path).await?;

        let status = resp.status();

        match status {
            StatusCode::OK | StatusCode::NOT_FOUND => Ok(RpDelete::default()),
            _ => Err(parse_error(resp).await?),
        }
    }

    async fn list(&self, path: &str, args: OpList) -> Result<(RpList, Self::Lister)> {
        let l = AzdlsLister::new(self.core.clone(), path.to_string(), args.limit());

        Ok((RpList::default(), oio::PageLister::new(l)))
    }

    async fn rename(&self, from: &str, to: &str, _args: OpRename) -> Result<RpRename> {
        if let Some(resp) = self.core.azdls_ensure_parent_path(to).await? {
            let status = resp.status();
            match status {
                StatusCode::CREATED | StatusCode::CONFLICT => {}
                _ => return Err(parse_error(resp).await?),
            }
        }

        let resp = self.core.azdls_rename(from, to).await?;

        let status = resp.status();

        match status {
            StatusCode::CREATED => Ok(RpRename::default()),
            _ => Err(parse_error(resp).await?),
        }
    }
}

fn infer_storage_name_from_endpoint(endpoint: &str) -> Option<String> {
    let endpoint: &str = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);

    let mut parts = endpoint.splitn(2, '.');
    let storage_name = parts.next();
    let endpoint_suffix = parts
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_lowercase();

    if KNOWN_AZDLS_ENDPOINT_SUFFIX
        .iter()
        .any(|s| *s == endpoint_suffix.as_str())
    {
        storage_name.map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::infer_storage_name_from_endpoint;

    #[test]
    fn test_infer_storage_name_from_endpoint() {
        let endpoint = "https://account.dfs.core.windows.net";
        let storage_name = infer_storage_name_from_endpoint(endpoint);
        assert_eq!(storage_name, Some("account".to_string()));
    }

    #[test]
    fn test_infer_storage_name_from_endpoint_with_trailing_slash() {
        let endpoint = "https://account.dfs.core.windows.net/";
        let storage_name = infer_storage_name_from_endpoint(endpoint);
        assert_eq!(storage_name, Some("account".to_string()));
    }
}

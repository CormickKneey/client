/*
 *     Copyright 2024 The Dragonfly Authors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use dragonfly_api::common;
use dragonfly_client_core::error::BackendError;
use dragonfly_client_core::{Error as ClientError, Result as ClientResult};
use opendal::{raw::HttpClient, Metakey, Operator};
use percent_encoding::percent_decode_str;
use std::fmt;
use std::result::Result;
use std::str::FromStr;
use std::time::Duration;
use tokio_util::io::StreamReader;
use tracing::{error, info};
use url::Url;

// Scheme is the scheme of the object storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    // S3 is the Amazon Simple Storage Service.
    S3,

    // GCS is the Google Cloud Storage Service.
    GCS,

    // ABS is the Azure Blob Storage Service.
    ABS,

    // OSS is the Aliyun Object Storage Service.
    OSS,

    // OBS is the Huawei Cloud Object Storage Service.
    OBS,

    // COS is the Tencent Cloud Object Storage Service.
    COS,
}

// Scheme implements the Display.
impl fmt::Display for Scheme {
    // fmt formats the value using the given formatter.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scheme::S3 => write!(f, "s3"),
            Scheme::GCS => write!(f, "gcs"),
            Scheme::ABS => write!(f, "abs"),
            Scheme::OSS => write!(f, "oss"),
            Scheme::OBS => write!(f, "obs"),
            Scheme::COS => write!(f, "cos"),
        }
    }
}

// Scheme implements the FromStr.
impl FromStr for Scheme {
    type Err = String;

    // from_str parses an scheme string.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "s3" => Ok(Scheme::S3),
            "gcs" => Ok(Scheme::GCS),
            "abs" => Ok(Scheme::ABS),
            "oss" => Ok(Scheme::OSS),
            "obs" => Ok(Scheme::OBS),
            "cos" => Ok(Scheme::COS),
            _ => Err(format!("invalid scheme: {}", s)),
        }
    }
}

// ParsedURL is a struct that contains the parsed URL, bucket, and path.
#[derive(Debug)]
pub struct ParsedURL {
    // url is the requested URL of the object storage.
    pub url: Url,

    // scheme is the scheme of the object storage.
    pub scheme: Scheme,

    // bucket is the bucket of the object storage.
    pub bucket: String,

    // key is the key of the object storage.
    pub key: String,
}

// ParsedURL implements the ParsedURL trait.
impl ParsedURL {
    // is_dir returns true if the URL path ends with a slash.
    pub fn is_dir(&self) -> bool {
        self.url.path().ends_with('/')
    }

    // make_url_by_entry_path makes a URL by the entry path when the URL is a directory.
    pub fn make_url_by_entry_path(&self, entry_path: &str) -> Url {
        let mut url = self.url.clone();
        url.set_path(entry_path);
        url
    }
}

// ParsedURL implements the TryFrom trait for the URL.
//
// The object storage URL should be in the format of `scheme://<bucket>/<path>`.
impl TryFrom<Url> for ParsedURL {
    type Error = ClientError;

    // try_from parses the URL and returns a ParsedURL.
    fn try_from(url: Url) -> Result<Self, Self::Error> {
        // Get the bucket from the URL host.
        let bucket = url
            .host_str()
            .ok_or_else(|| ClientError::InvalidURI(url.to_string()))?
            .to_string();

        // Get the scheme from the URL scheme.
        let scheme: Scheme = url.scheme().to_string().parse().map_err(|err| {
            error!("parse scheme failed {}: {}", url, err);
            ClientError::InvalidURI(url.to_string())
        })?;

        // Get the key from the URL path.
        let key = url
            .path()
            .strip_prefix('/')
            .ok_or_else(|| ClientError::InvalidURI(url.to_string()))?;
        // Decode the key.
        let decoded_key = percent_decode_str(key).decode_utf8_lossy().to_string();

        Ok(Self {
            url,
            scheme,
            bucket,
            key: decoded_key,
        })
    }
}

// ObjectStorage is a struct that implements the backend trait.
pub struct ObjectStorage {
    // scheme is the scheme of the object storage.
    scheme: Scheme,
}

// ObjectStorage implements the ObjectStorage trait.
impl ObjectStorage {
    /// Returns ObjectStorage that implements the Backend trait.
    pub fn new(scheme: Scheme) -> ObjectStorage {
        Self { scheme }
    }

    // operator initializes the operator with the parsed URL and object storage.
    pub fn operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        match self.scheme {
            Scheme::S3 => self.s3_operator(parsed_url, object_storage, timeout),
            Scheme::GCS => self.gcs_operator(parsed_url, object_storage, timeout),
            Scheme::ABS => self.abs_operator(parsed_url, object_storage, timeout),
            Scheme::OSS => self.oss_operator(parsed_url, object_storage, timeout),
            Scheme::OBS => self.obs_operator(parsed_url, object_storage, timeout),
            Scheme::COS => self.cos_operator(parsed_url, object_storage, timeout),
        }
    }

    // s3_operator initializes the S3 operator with the parsed URL and object storage.
    pub fn s3_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need access_key_id and access_key_secret");
            return Err(ClientError::BackendError(BackendError {
                message: "need access_key_id and access_key_secret".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the S3 operator with the object storage.
        let mut builder = opendal::services::S3::default();
        builder
            .access_key_id(&object_storage.access_key_id)
            .secret_access_key(&object_storage.access_key_secret)
            .http_client(HttpClient::with(client))
            .bucket(&parsed_url.bucket);

        // Configure the region and endpoint if they are provided.
        if let Some(region) = object_storage.region.as_deref() {
            builder.region(region);
        }

        // Configure the endpoint if it is provided.
        if let Some(endpoint) = object_storage.endpoint.as_deref() {
            builder.endpoint(endpoint);
        }

        // Configure the session token if it is provided.
        if let Some(session_token) = object_storage.session_token.as_deref() {
            builder.security_token(session_token);
        }

        Ok(Operator::new(builder)?.finish())
    }

    // gcs_operator initializes the GCS operator with the parsed URL and object storage.
    pub fn gcs_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need credential");
            return Err(ClientError::BackendError(BackendError {
                message: "need credential".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the GCS operator with the object storage.
        let mut builder = opendal::services::Gcs::default();
        builder
            .http_client(HttpClient::with(client))
            .bucket(&parsed_url.bucket);

        // Configure the region and endpoint if they are provided.
        if let Some(credential) = object_storage.credential.as_deref() {
            builder.credential(credential);
        } else {
            error!("need credential");
            return Err(ClientError::BackendError(BackendError {
                message: "need credential".to_string(),
                status_code: None,
                header: None,
            }));
        }

        // Configure the predefined ACL if it is provided.
        if let Some(predefined_acl) = object_storage.predefined_acl.as_deref() {
            builder.predefined_acl(predefined_acl);
        }

        Ok(Operator::new(builder)?.finish())
    }

    // abs_operator initializes the ABS operator with the parsed URL and object storage.
    pub fn abs_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need access_key_id and access_key_secret");
            return Err(ClientError::BackendError(BackendError {
                message: "need access_key_id and access_key_secret".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the ABS operator with the object storage.
        let mut builder = opendal::services::Azblob::default();
        builder
            .account_name(&object_storage.access_key_id)
            .account_key(&object_storage.access_key_secret)
            .http_client(HttpClient::with(client))
            .container(&parsed_url.bucket);

        // Configure the endpoint if it is provided.
        if let Some(endpoint) = object_storage.endpoint.as_deref() {
            builder.endpoint(endpoint);
        }

        Ok(Operator::new(builder)?.finish())
    }

    // oss_operator initializes the OSS operator with the parsed URL and object storage.
    pub fn oss_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need endpoint, access_key_id and access_key_secret");
            return Err(ClientError::BackendError(BackendError {
                message: "need endpoint, access_key_id and access_key_secret".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the OSS operator with the object storage.
        let mut builder = opendal::services::Oss::default();
        builder
            .access_key_id(&object_storage.access_key_id)
            .access_key_secret(&object_storage.access_key_secret)
            .http_client(HttpClient::with(client))
            .root("/")
            .bucket(&parsed_url.bucket);

        // Configure the endpoint if provided.
        if let Some(endpoint) = object_storage.endpoint {
            builder.endpoint(&endpoint);
        }

        Ok(Operator::new(builder)?.finish())
    }

    // obs_operator initializes the OBS operator with the parsed URL and object storage.
    pub fn obs_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need endpoint, access_key_id and access_key_secret");
            return Err(ClientError::BackendError(BackendError {
                message: "need endpoint, access_key_id and access_key_secret".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the OBS operator with the object storage.
        let mut builder = opendal::services::Obs::default();
        builder
            .access_key_id(&object_storage.access_key_id)
            .secret_access_key(&object_storage.access_key_secret)
            .http_client(HttpClient::with(client))
            .bucket(&parsed_url.bucket);

        // Configure the endpoint if provided.
        if let Some(endpoint) = object_storage.endpoint {
            builder.endpoint(&endpoint);
        }

        Ok(Operator::new(builder)?.finish())
    }

    // cos_operator initializes the COS operator with the parsed URL and object storage.
    pub fn cos_operator(
        &self,
        parsed_url: &super::object_storage::ParsedURL,
        object_storage: Option<common::v2::ObjectStorage>,
        timeout: Duration,
    ) -> ClientResult<Operator> {
        // Check if the object storage is provided.
        let Some(object_storage) = object_storage else {
            error!("need endpoint, access_key_id and access_key_secret");
            return Err(ClientError::BackendError(BackendError {
                message: "need endpoint, access_key_id and access_key_secret".to_string(),
                status_code: None,
                header: None,
            }));
        };

        // Create a reqwest http client.
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        // Initialize the COS operator with the object storage.
        let mut builder = opendal::services::Cos::default();
        builder
            .secret_id(&object_storage.access_key_id)
            .secret_key(&object_storage.access_key_secret)
            .http_client(HttpClient::with(client))
            .bucket(&parsed_url.bucket);

        // Configure the endpoint if provided.
        if let Some(endpoint) = object_storage.endpoint {
            builder.endpoint(&endpoint);
        }

        Ok(Operator::new(builder)?.finish())
    }
}

// Backend implements the Backend trait.
#[tonic::async_trait]
impl crate::Backend for ObjectStorage {
    // scheme returns the scheme of the object storage.
    fn scheme(&self) -> String {
        self.scheme.to_string()
    }

    //head gets the header of the request.
    async fn head(&self, request: super::HeadRequest) -> ClientResult<super::HeadResponse> {
        info!(
            "head request {} {}: {:?}",
            request.task_id, request.url, request.http_header
        );

        // Parse the URL and convert it to a ParsedURL for create the ObjectStorage operator.
        let url: Url = request
            .url
            .parse()
            .map_err(|_| ClientError::InvalidURI(request.url.clone()))?;
        let parsed_url: super::object_storage::ParsedURL = url.try_into().map_err(|err| {
            error!(
                "parse head request url failed {} {}: {}",
                request.task_id, request.url, err
            );
            err
        })?;

        // Initialize the operator with the parsed URL, object storage, and timeout.
        let operator = self.operator(&parsed_url, request.object_storage, request.timeout)?;

        // Get the entries if url point to a directory.
        let entries = if parsed_url.is_dir() {
            operator
                .list_with(&parsed_url.key)
                .recursive(true)
                .metakey(Metakey::ContentLength | Metakey::Mode)
                .await // Do the list op here.
                .map_err(|err| {
                    error!(
                        "list request failed {} {}: {}",
                        request.task_id, request.url, err
                    );
                    ClientError::BackendError(BackendError {
                        message: err.to_string(),
                        status_code: None,
                        header: None,
                    })
                })?
                .into_iter()
                .map(|entry| {
                    let metadata = entry.metadata();
                    super::DirEntry {
                        url: parsed_url.make_url_by_entry_path(entry.path()).to_string(),
                        content_length: metadata.content_length() as usize,
                        is_dir: metadata.is_dir(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Stat the object to get the response from the ObjectStorage.
        let response = operator.stat_with(&parsed_url.key).await.map_err(|err| {
            error!(
                "stat request failed {} {}: {}",
                request.task_id, request.url, err
            );
            ClientError::BackendError(BackendError {
                message: err.to_string(),
                status_code: None,
                header: None,
            })
        })?;

        info!(
            "head response {} {}: {}",
            request.task_id,
            request.url,
            response.content_length()
        );

        Ok(super::HeadResponse {
            success: true,
            content_length: Some(response.content_length()),
            http_header: None,
            http_status_code: None,
            error_message: None,
            entries,
        })
    }

    // Returns content of requested file.
    async fn get(
        &self,
        request: super::GetRequest,
    ) -> ClientResult<super::GetResponse<super::Body>> {
        info!(
            "get request {} {}: {:?}",
            request.piece_id, request.url, request.http_header
        );

        // Parse the URL and convert it to a ParsedURL for create the ObjectStorage operator.
        let url: Url = request
            .url
            .parse()
            .map_err(|_| ClientError::InvalidURI(request.url.clone()))?;
        let parsed_url: super::object_storage::ParsedURL = url.try_into().map_err(|err| {
            error!(
                "parse head request url failed {} {}: {}",
                request.piece_id, request.url, err
            );
            err
        })?;

        // Initialize the operator with the parsed URL, object storage, and timeout.
        let operator_reader = self
            .operator(&parsed_url, request.object_storage, request.timeout)?
            .reader(&parsed_url.key)
            .await
            .map_err(|err| {
                error!(
                    "get request failed {} {}: {}",
                    request.piece_id, request.url, err
                );
                ClientError::BackendError(BackendError {
                    message: err.to_string(),
                    status_code: None,
                    header: None,
                })
            })?;

        let stream = match request.range {
            Some(range) => operator_reader
                .into_bytes_stream(range.start..range.start + range.length)
                .await
                .map_err(|err| {
                    error!(
                        "get request failed {} {}: {}",
                        request.piece_id, request.url, err
                    );
                    ClientError::BackendError(BackendError {
                        message: err.to_string(),
                        status_code: None,
                        header: None,
                    })
                })?,
            None => operator_reader.into_bytes_stream(..).await.map_err(|err| {
                error!(
                    "get request failed {} {}: {}",
                    request.piece_id, request.url, err
                );
                ClientError::BackendError(BackendError {
                    message: err.to_string(),
                    status_code: None,
                    header: None,
                })
            })?,
        };

        Ok(crate::GetResponse {
            success: true,
            http_header: None,
            http_status_code: Some(reqwest::StatusCode::OK),
            reader: Box::new(StreamReader::new(stream)),
            error_message: None,
        })
    }
}

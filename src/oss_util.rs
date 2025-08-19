use std::{path::PathBuf, str::FromStr};

use reqwest::{header::HeaderMap, Body};
use serde::Deserialize;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use url::Url;

/// Helper function to convert any Display error to DashScopeError::UploadError
fn upload_err<E: std::fmt::Display>(e: E) -> crate::error::DashScopeError {
    crate::error::DashScopeError::UploadError(e.to_string())
}

/// Create HTTP client with proper timeout configuration
fn new_http_client() -> reqwest::Client {
    use std::time::Duration;
    reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(60))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("HTTP client should build successfully")
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UploadPolicy {
    pub(crate) data: PolicyData,
    pub(crate) request_id: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PolicyData {
    pub(crate) policy: String,
    pub(crate) signature: String,
    pub(crate) upload_dir: String,
    pub(crate) upload_host: String,
    pub(crate) expire_in_seconds: i64,
    pub(crate) max_file_size_mb: i32,
    pub(crate) capacity_limit_mb: i32,
    pub(crate) oss_access_key_id: String,
    pub(crate) x_oss_object_acl: String,
    pub(crate) x_oss_forbid_overwrite: String,
}

/// 获取文件上传凭证（使用提供的HTTP客户端）
pub(crate) async fn get_upload_policy_with_client(
    http_client: &reqwest::Client,
    api_key: &str,
    model_name: &str,
) -> Result<UploadPolicy, crate::error::DashScopeError> {
    let url = "https://dashscope.aliyuncs.com/api/v1/uploads";
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {api_key}")
            .parse()
            .expect("authorization header should parse successfully"),
    );
    headers.insert(
        "Content-Type",
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    let params = json!({
        "action": "getPolicy",
        "model": model_name
    });
    http_client
        .get(url)
        .headers(headers)
        .query(&params)
        .send()
        .await
        .map_err(upload_err)?
        .json::<UploadPolicy>()
        .await
        .map_err(upload_err)
}

/// 获取文件上传凭证（使用共享HTTP客户端）
pub(crate) async fn get_upload_policy(
    api_key: &str,
    model_name: &str,
) -> Result<UploadPolicy, crate::error::DashScopeError> {
    use std::sync::OnceLock;
    static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

    let client = SHARED_CLIENT.get_or_init(new_http_client);
    get_upload_policy_with_client(client, api_key, model_name).await
}

/// 获取共享的 OSS 上传客户端
fn oss_http_client() -> &'static reqwest::Client {
    use std::sync::OnceLock;
    static OSS_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    OSS_CLIENT.get_or_init(new_http_client)
}

/// 将文件上传到临时存储OSS
pub(crate) async fn upload_file_to_oss(
    policy_data: PolicyData,
    file: File,
    file_name: &str,
) -> Result<String, crate::error::DashScopeError> {
    let meta = file
        .metadata()
        .await
        .map_err(upload_err)?;
    let len = meta.len();

    let key = format!("{}/{}", policy_data.upload_dir, file_name);

    let stream = ReaderStream::new(file);
    let body = Body::wrap_stream(stream);
    let part = if len > 0 {
        reqwest::multipart::Part::stream_with_length(body, len)
            .file_name(file_name.to_string())
    } else {
        // 部分服务端对 chunked 可兼容
        reqwest::multipart::Part::stream(body).file_name(file_name.to_string())
    };

    let form = reqwest::multipart::Form::new()
        .text("OSSAccessKeyId", policy_data.oss_access_key_id.clone())
        .text("Signature", policy_data.signature.clone())
        .text("policy", policy_data.policy.clone())
        .text("x-oss-object-acl", policy_data.x_oss_object_acl.clone())
        .text(
            "x-oss-forbid-overwrite",
            policy_data.x_oss_forbid_overwrite.clone(),
        )
        .text("key", key.clone())
        .text("success_action_status", "200".to_string())
        .part("file", part);

    let response = oss_http_client()
        .post(&policy_data.upload_host)
        .multipart(form)
        .send()
        .await
        .map_err(upload_err)?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(crate::error::DashScopeError::UploadError(text));
    }

    Ok(format!("oss://{key}"))
}

pub(crate) async fn upload_file_and_get_url(
    api_key: &str,
    model_name: &str,
    file_path: &str,
) -> Result<String, crate::error::DashScopeError> {
    let p = PathBuf::from_str(file_path)
        .map_err(upload_err)?;

    let file_name = p
        .file_name()
        .ok_or_else(|| upload_err("file name is empty"))?
        .to_str()
        .ok_or_else(|| upload_err("file name is not valid UTF-8"))?;

    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(file_path)
        .await
        .map_err(upload_err)?;
    let meta = file
        .metadata()
        .await
        .map_err(upload_err)?;

    if !meta.is_file() {
        return Err(crate::error::DashScopeError::UploadError(
            "file is not a file".into(),
        ));
    }

    let policy_data = get_upload_policy(api_key, model_name).await?;

    let url = upload_file_to_oss(policy_data.data, file, file_name).await?;

    Ok(url)
}

/// 检查字符串是否为有效的URL (仅支持 http 和 https 协议)
pub(crate) fn is_valid_url(s: &str) -> bool {
    Url::parse(s)
        .map(|u| ["http", "https"].contains(&u.scheme()))
        .unwrap_or(false)
}

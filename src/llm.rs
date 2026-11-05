//! 自建大模型接入：OpenAI 兼容的 Chat Completions 接口（例如内网部署的 gpt-oss-20b / 120b）。
//!
//! - 地址、模型等配置保存在数据目录的 `llm.txt`；API Key 只保存在 Windows 凭据管理器。
//! - Windows 上 HTTPS 使用系统证书库，已导入系统的内网企业 CA 证书无需额外配置。
//! - 只读取最终回答 `message.content`；gpt-oss 的推理过程（`reasoning_content` 等字段）不会返回给调用方。
//! - 不跟随跳转、不走系统代理，避免把 API Key 发到内网服务以外的地址。
//! - [`Client`] 的请求都是阻塞调用，只能在后台线程中使用。

use serde_json::{json, Value};
use std::io::Read;
use std::time::{Duration, Instant};

const CONFIG_FILE: &str = "llm.txt";
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
const MAX_ERROR_DETAIL: usize = 200;

/// gpt-oss 的推理强度。`Unset` 表示不发送该参数，由服务端决定。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Effort {
    Unset,
    #[default]
    Low,
    Medium,
    High,
}

impl Effort {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Unset];

    pub fn key(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unset => "不指定",
            Self::Low => "低（响应最快）",
            Self::Medium => "中",
            Self::High => "高（最慢）",
        }
    }

    fn parse(value: &str) -> Self {
        Self::ALL.into_iter().find(|effort| effort.key() == value).unwrap_or_default()
    }

    fn param(self) -> Option<&'static str> {
        (self != Self::Unset).then(|| self.key())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// 总开关；关闭时 [`Client::from_saved`] 拒绝创建客户端，不会发送任何内容。
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub effort: Effort,
    pub timeout_secs: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            model: "gpt-oss-20b".into(),
            effort: Effort::Low,
            timeout_secs: 60,
        }
    }
}

impl Config {
    pub const TIMEOUT_RANGE: std::ops::RangeInclusive<u32> = 10..=600;

    pub fn load() -> Self {
        let mut config = Self::default();
        for (key, value) in crate::model::read_kv(&crate::model::config_dir().join(CONFIG_FILE)) {
            match key.as_str() {
                "enabled" => config.enabled = value == "1",
                "base_url" => config.base_url = value,
                "model" => config.model = value,
                "effort" => config.effort = Effort::parse(&value),
                "timeout_secs" => {
                    let range = Self::TIMEOUT_RANGE;
                    config.timeout_secs = value.parse().unwrap_or(60).clamp(*range.start(), *range.end());
                }
                _ => {}
            }
        }
        config
    }

    pub fn save(&self) -> std::io::Result<()> {
        let clean = |value: &str| value.trim().replace(['\r', '\n'], "");
        crate::model::write_kv(
            &crate::model::config_dir().join(CONFIG_FILE),
            &[
                ("enabled".into(), (self.enabled as u8).to_string()),
                ("base_url".into(), clean(&self.base_url)),
                ("model".into(), clean(&self.model)),
                ("effort".into(), self.effort.key().into()),
                ("timeout_secs".into(), self.timeout_secs.to_string()),
            ],
        )
    }
}

/// 由用户填写的地址得到具体接口地址。
///
/// 接受 `http://host:8000`、`http://host:8000/v1`，以及直接粘贴的完整接口地址
/// （以 `/chat/completions` 或 `/models` 结尾）；路径为空时默认使用 `/v1`。
pub fn endpoint(base: &str, path: &str) -> Result<url::Url, String> {
    let base = base.trim();
    if base.is_empty() {
        return Err("请先填写接口地址".into());
    }
    let base = if base.contains("://") { base.to_string() } else { format!("http://{base}") };
    let mut url = url::Url::parse(&base).map_err(|_| "接口地址格式不对，例如 http://10.0.0.5:8000/v1".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("接口地址需以 http:// 或 https:// 开头".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("地址里不要包含账号密码，请把密钥填到 API Key".into());
    }
    let mut prefix = url.path().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/models"] {
        if let Some(stripped) = prefix.strip_suffix(suffix) {
            prefix = stripped.to_string();
            break;
        }
    }
    if prefix.is_empty() {
        prefix = "/v1".into();
    }
    url.set_path(&format!("{prefix}/{path}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system", content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user", content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant", content: content.into() }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// 生成上限。gpt-oss 的推理内容也计入该上限，设得太小会只剩推理、没有回答。
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// 要求服务端只返回 JSON 对象（`response_format: json_object`）。
    pub json: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { max_tokens: 1024, temperature: None, json: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// 最终回答，已去掉推理内容和首尾空白。
    pub content: String,
    /// 服务端实际使用的模型名。
    pub model: String,
    pub elapsed: Duration,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

pub struct Client {
    config: Config,
    api_key: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(config: Config, api_key: Option<String>) -> Result<Self, String> {
        endpoint(&config.base_url, "chat/completions")?;
        if config.model.trim().is_empty() {
            return Err("请先填写模型名称".into());
        }
        let api_key = api_key.map(|key| key.trim().to_string()).filter(|key| !key.is_empty());
        let agent = agent(Duration::from_secs(config.timeout_secs.into()))?;
        Ok(Self { config, api_key, agent })
    }

    /// 使用已保存的配置和 API Key；未启用或未配置时返回错误。
    pub fn from_saved() -> Result<Self, String> {
        let config = Config::load();
        if !config.enabled {
            return Err("大模型功能未启用".into());
        }
        Self::new(config, secrets::load())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn chat(&self, messages: &[Message], options: &Options) -> Result<Reply, String> {
        let url = endpoint(&self.config.base_url, "chat/completions")?;
        let mut body = json!({
            "model": self.config.model.trim(),
            "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "max_tokens": options.max_tokens,
            "stream": false,
        });
        if let Some(effort) = self.config.effort.param() {
            body["reasoning_effort"] = effort.into();
        }
        if let Some(temperature) = options.temperature {
            body["temperature"] = temperature.into();
        }
        if options.json {
            body["response_format"] = json!({"type": "json_object"});
        }
        let started = Instant::now();
        let request = self.authorize(self.agent.post(url.as_str())).set("Content-Type", "application/json");
        let value = read_json(request.send_string(&body.to_string()))?;
        let mut reply = parse_reply(&value)?;
        reply.elapsed = started.elapsed();
        Ok(reply)
    }

    /// 服务端可用的模型名称（`GET /models`），已排序去重。
    pub fn models(&self) -> Result<Vec<String>, String> {
        let url = endpoint(&self.config.base_url, "models")?;
        let value = read_json(self.authorize(self.agent.get(url.as_str())).call())?;
        let mut ids: Vec<String> = value["data"]
            .as_array()
            .ok_or("服务没有返回模型列表")?
            .iter()
            .filter_map(|model| model["id"].as_str())
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// 设置页的「测试连接」：确认地址、密钥和模型名可用，并实际生成一次回答。
    pub fn test(&self) -> Result<String, String> {
        // 部分网关不提供 /models，此时直接用对话请求验证。
        if let Ok(models) = self.models() {
            let model = self.config.model.trim();
            if !models.is_empty() && !models.iter().any(|name| name == model) {
                return Err(format!("服务上没有模型「{model}」，可用：{}", models.join("、")));
            }
        }
        let reply = self.chat(
            &[Message::system("你是连接测试助手。只回复 OK，不要输出其他内容。"), Message::user("ping")],
            &Options { max_tokens: 512, ..Default::default() },
        )?;
        let preview: String = reply.content.chars().take(40).collect();
        Ok(format!("连接成功 · {} · 用时 {:.1} 秒 · 回复：{preview}", reply.model, reply.elapsed.as_secs_f32()))
    }

    fn authorize(&self, request: ureq::Request) -> ureq::Request {
        let request = request.set("Accept", "application/json");
        match &self.api_key {
            Some(key) => request.set("Authorization", &format!("Bearer {key}")),
            None => request,
        }
    }
}

fn agent(timeout: Duration) -> Result<ureq::Agent, String> {
    let builder = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(timeout)
        .timeout_write(Duration::from_secs(30))
        .user_agent(concat!(crate::app_id!(), "/", env!("CARGO_PKG_VERSION")));
    // 使用 Windows 证书库校验 HTTPS，内网 CA 导入系统后即可使用。
    #[cfg(windows)]
    let builder = builder.tls_connector(std::sync::Arc::new(
        native_tls::TlsConnector::new().map_err(|error| format!("无法初始化 HTTPS：{error}"))?,
    ));
    Ok(builder.build())
}

fn read_json(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    let response = result.map_err(describe)?;
    if (300..400).contains(&response.status()) {
        return Err(format!("服务返回了跳转（{}），请直接填写最终的接口地址", response.status()));
    }
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_string(&mut text)
        .map_err(|error| io_error_text(&error))?;
    if text.len() as u64 > MAX_RESPONSE {
        return Err("服务响应过大".into());
    }
    serde_json::from_str(&text).map_err(|_| "服务返回的不是 JSON，请确认地址指向 OpenAI 兼容接口".into())
}

fn io_error_text(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            "等待模型响应超时，可调高超时时间或降低推理强度".into()
        }
        _ => format!("读取响应失败：{error}"),
    }
}

fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let head = match code {
                401 | 403 => "API Key 无效或没有权限",
                404 => "接口地址或模型名称不对",
                429 => "请求过于频繁，请稍后再试",
                400 | 422 => "请求被模型服务拒绝",
                500..=599 => "模型服务出错",
                _ => "请求失败",
            };
            match server_message(response) {
                Some(detail) => format!("{head}（{code}）：{detail}"),
                None => format!("{head}（{code}）"),
            }
        }
        ureq::Error::Transport(transport) => {
            let source = std::error::Error::source(&transport);
            if let Some(io) = source.and_then(|error| error.downcast_ref::<std::io::Error>()) {
                if matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                    return "连接或等待模型服务超时，请确认已连接内网或 VPN".into();
                }
            }
            if transport.message() == Some("native_tls connect failed") {
                let detail = source.map(|error| error.to_string()).unwrap_or_default();
                return format!("HTTPS 证书校验失败，请确认内网证书已导入系统：{}", truncate(detail.trim()));
            }
            match transport.kind() {
                ureq::ErrorKind::Dns => "找不到服务器地址，请确认已连接内网或 VPN".into(),
                ureq::ErrorKind::ConnectionFailed => {
                    "无法连接到模型服务，请确认已连接内网或 VPN，并检查地址和端口".into()
                }
                ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => "接口地址格式不对".into(),
                _ => format!("连接失败：{}", truncate(&transport.to_string())),
            }
        }
    }
}

/// OpenAI 兼容服务的错误详情，常见格式为 `{"error":{"message":…}}`、`{"error":"…"}` 或 `{"detail":"…"}`。
fn server_message(response: ureq::Response) -> Option<String> {
    let mut text = String::new();
    response.into_reader().take(64 * 1024).read_to_string(&mut text).ok()?;
    let detail = match serde_json::from_str::<Value>(&text) {
        Ok(value) => [&value["error"]["message"], &value["error"], &value["message"], &value["detail"]]
            .into_iter()
            .find_map(|field| field.as_str().map(str::to_owned))?,
        Err(_) => text,
    };
    let detail = detail.trim();
    (!detail.is_empty()).then(|| truncate(detail))
}

fn truncate(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(MAX_ERROR_DETAIL).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn parse_reply(value: &Value) -> Result<Reply, String> {
    let choice = value["choices"].get(0).ok_or("模型服务没有返回回答")?;
    let content = strip_reasoning(choice["message"]["content"].as_str().unwrap_or_default());
    if content.is_empty() {
        return Err(if choice["finish_reason"].as_str() == Some("length") {
            "回答被截断：推理用完了生成上限，请提高上限或降低推理强度".into()
        } else {
            "模型没有返回回答内容".into()
        });
    }
    Ok(Reply {
        content,
        model: value["model"].as_str().unwrap_or_default().to_string(),
        elapsed: Duration::ZERO,
        prompt_tokens: value["usage"]["prompt_tokens"].as_u64(),
        completion_tokens: value["usage"]["completion_tokens"].as_u64(),
    })
}

/// 部分部署会把推理过程用 `<think>…</think>` 混在正文里，这里一并去掉。
fn strip_reasoning(content: &str) -> String {
    let mut rest = content;
    let mut out = String::new();
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// API Key 只保存在 Windows 凭据管理器。
pub mod secrets {
    pub const SERVICE: &str = concat!(crate::app_id!(), ".LLM");
    const USER: &str = "default";

    #[cfg(windows)]
    pub fn save(key: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, USER)
            .and_then(|entry| entry.set_password(key.trim()))
            .map_err(|error| format!("保存 API Key 失败：{error}"))
    }

    #[cfg(windows)]
    pub fn load() -> Option<String> {
        keyring::Entry::new(SERVICE, USER).ok()?.get_password().ok().filter(|key| !key.trim().is_empty())
    }

    #[cfg(windows)]
    pub fn delete() {
        if let Ok(entry) = keyring::Entry::new(SERVICE, USER) {
            let _ = entry.delete_credential();
        }
    }

    #[cfg(not(windows))]
    pub fn save(_key: &str) -> Result<(), String> {
        Err("仅 Windows 支持凭据管理器".into())
    }

    #[cfg(not(windows))]
    pub fn load() -> Option<String> {
        None
    }

    #[cfg(not(windows))]
    pub fn delete() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// 只接收一次请求的本地 HTTP 服务；返回地址和收到的原始请求。
    fn serve_once(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (address, worker)
    }

    fn client(base_url: &str, key: Option<&str>) -> Client {
        let config = Config { enabled: true, base_url: base_url.into(), timeout_secs: 10, ..Default::default() };
        Client::new(config, key.map(str::to_owned)).unwrap()
    }

    #[test]
    fn endpoint_accepts_common_base_url_forms() {
        for (base, expected) in [
            ("http://10.0.0.5:8000", "http://10.0.0.5:8000/v1/chat/completions"),
            ("http://10.0.0.5:8000/", "http://10.0.0.5:8000/v1/chat/completions"),
            ("http://10.0.0.5:8000/v1", "http://10.0.0.5:8000/v1/chat/completions"),
            ("http://10.0.0.5:8000/v1/", "http://10.0.0.5:8000/v1/chat/completions"),
            ("http://10.0.0.5:8000/v1/chat/completions", "http://10.0.0.5:8000/v1/chat/completions"),
            ("https://llm.corp.local/api/v1/models", "https://llm.corp.local/api/v1/chat/completions"),
            ("  10.0.0.5:8000/v1?x=1  ", "http://10.0.0.5:8000/v1/chat/completions"),
            ("llm.corp.local", "http://llm.corp.local/v1/chat/completions"),
        ] {
            assert_eq!(endpoint(base, "chat/completions").unwrap().as_str(), expected, "{base}");
        }
        assert_eq!(endpoint("http://h:8000/v1", "models").unwrap().as_str(), "http://h:8000/v1/models");
        for bad in ["", "   ", "ftp://h/v1", "http://user:pass@h/v1", "http://"] {
            assert!(endpoint(bad, "chat/completions").is_err(), "{bad}");
        }
    }

    #[test]
    fn reply_ignores_reasoning_and_reports_truncation() {
        let value = json!({"model":"gpt-oss-20b","choices":[{"message":{"content":"  OK ","reasoning_content":"用户要我回复 OK"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":12,"completion_tokens":30}});
        let reply = parse_reply(&value).unwrap();
        assert_eq!(reply.content, "OK");
        assert_eq!(reply.model, "gpt-oss-20b");
        assert_eq!((reply.prompt_tokens, reply.completion_tokens), (Some(12), Some(30)));

        let think = json!({"choices":[{"message":{"content":"<think>先想想</think>\n答案是 42"}}]});
        assert_eq!(parse_reply(&think).unwrap().content, "答案是 42");
        assert_eq!(strip_reasoning("<think>未闭合的推理"), "");
        assert_eq!(strip_reasoning("前<think>a</think>中<think>b</think>后"), "前中后");

        let truncated = json!({"choices":[{"message":{"content":null,"reasoning_content":"思考中…"},"finish_reason":"length"}]});
        assert!(parse_reply(&truncated).unwrap_err().contains("截断"));
        assert!(parse_reply(&json!({"choices":[]})).is_err());
    }

    #[test]
    fn chat_sends_openai_compatible_request() {
        let (address, worker) = serve_once(
            "200 OK",
            r#"{"model":"gpt-oss-20b","choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}]}"#,
        );
        let reply = client(&format!("{address}/v1"), Some(" sk-test "))
            .chat(&[Message::system("系统"), Message::user("问题")], &Options { json: true, ..Default::default() })
            .unwrap();
        assert_eq!(reply.content, "你好");
        let request = worker.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"), "{request}");
        assert!(request.contains("Authorization: Bearer sk-test\r\n"));
        let body: Value = serde_json::from_str(&request[request.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["model"], "gpt-oss-20b");
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["stream"], false);
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["messages"][0], json!({"role":"system","content":"系统"}));
        assert_eq!(body["messages"][1], json!({"role":"user","content":"问题"}));
    }

    #[test]
    fn requests_without_key_or_effort_omit_them() {
        let (address, worker) = serve_once("200 OK", r#"{"choices":[{"message":{"content":"OK"}}]}"#);
        let config = Config { base_url: address, effort: Effort::Unset, ..Default::default() };
        Client::new(config, Some("  ".into())).unwrap().chat(&[Message::user("hi")], &Options::default()).unwrap();
        let request = worker.join().unwrap();
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert!(!request.contains("reasoning_effort"));
    }

    #[test]
    fn models_are_sorted_and_deduplicated() {
        let (address, worker) = serve_once("200 OK", r#"{"data":[{"id":"gpt-oss-120b"},{"id":"gpt-oss-20b"},{"id":"gpt-oss-120b"}]}"#);
        assert_eq!(client(&address, None).models().unwrap(), ["gpt-oss-120b", "gpt-oss-20b"]);
        assert!(worker.join().unwrap().starts_with("GET /v1/models HTTP/1.1"));
    }

    #[test]
    fn errors_are_explained_in_chinese() {
        let (address, worker) = serve_once("401 Unauthorized", r#"{"error":{"message":"invalid api key"}}"#);
        let error = client(&address, Some("bad")).chat(&[Message::user("hi")], &Options::default()).unwrap_err();
        worker.join().unwrap();
        assert_eq!(error, "API Key 无效或没有权限（401）：invalid api key");

        let (address, worker) = serve_once("302 Found", "{}");
        let error = client(&address, Some("secret")).models().unwrap_err();
        worker.join().unwrap();
        assert!(error.contains("跳转"), "{error}");

        // 绑定后立即释放端口，确保连接被拒绝。
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let error = client(&format!("http://{closed}"), None).models().unwrap_err();
        assert!(error.contains("内网") || error.contains("连接"), "{error}");
    }

    #[test]
    fn test_reports_missing_model() {
        let (address, worker) = serve_once("200 OK", r#"{"data":[{"id":"gpt-oss-120b"}]}"#);
        let error = client(&address, None).test().unwrap_err();
        worker.join().unwrap();
        assert!(error.contains("gpt-oss-20b") && error.contains("gpt-oss-120b"), "{error}");
    }

    #[test]
    fn config_round_trips_and_clamps() {
        let dir = std::env::temp_dir().join(format!("daozai-llm-config-{}", std::process::id()));
        let old = std::env::var_os("DAOZAI_DATA_DIR");
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        assert_eq!(Config::load(), Config::default());
        let config = Config {
            enabled: true,
            base_url: " http://10.0.0.5:8000/v1\n".into(),
            model: "gpt-oss-120b".into(),
            effort: Effort::High,
            timeout_secs: 120,
        };
        config.save().unwrap();
        let loaded = Config::load();
        assert_eq!(loaded, Config { base_url: "http://10.0.0.5:8000/v1".into(), ..config });
        std::fs::write(dir.join(CONFIG_FILE), "timeout_secs=99999\neffort=bogus\n").unwrap();
        let loaded = Config::load();
        assert_eq!((loaded.timeout_secs, loaded.effort), (600, Effort::Low));
        match old {
            Some(old) => std::env::set_var("DAOZAI_DATA_DIR", old),
            None => std::env::remove_var("DAOZAI_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 连接真实服务：
    /// `DAOZAI_LLM_URL=http://10.0.0.5:8000/v1 DAOZAI_LLM_MODEL=gpt-oss-20b cargo test --lib llm::tests::live -- --ignored --nocapture`
    /// 可选 `DAOZAI_LLM_KEY`。
    #[test]
    #[ignore = "需要可访问的模型服务"]
    fn live() {
        let config = Config {
            enabled: true,
            base_url: std::env::var("DAOZAI_LLM_URL").expect("需设置 DAOZAI_LLM_URL"),
            model: std::env::var("DAOZAI_LLM_MODEL").unwrap_or_else(|_| "gpt-oss-20b".into()),
            ..Default::default()
        };
        let client = Client::new(config, std::env::var("DAOZAI_LLM_KEY").ok()).unwrap();
        println!("{}", client.test().unwrap());
    }
}

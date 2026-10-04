//! OpenAI 兼容 API 客户端（纯异步：reqwest 开启 stream 特性 + tokio）。
//!
//! 能力：
//! - 聊天补全 SSE 流式输出（[`OpenAiClient::chat_stream`]）；
//! - 结构化 JSON 输出（[`OpenAiClient::chat_json`]），
//!   服务于「角色铸造」（[`OpenAiClient::forge_character`]）与
//!   「记忆提炼」（[`OpenAiClient::refine_memory`]）；
//! - 安全约束（SSRF 防护）：
//!   - URL 仅接受 http/https；
//!   - 每次请求前重新解析并校验目标 host，拒绝 localhost、环回、私有、
//!     链路本地与其它保留地址（域名解析到任一禁用地址即整体拒绝，fail-closed）；
//!   - 禁止跟随重定向（防止公网地址 302 跳转到内网绕过校验）。
//!
//! 说明：host 校验里的 DNS 解析用标准库同步接口，调用频度极低，开销可忽略。

use std::collections::VecDeque;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::pin::Pin;
use std::time::Duration;

use futures_util::{Stream, stream};
use reqwest::{Client, Url, redirect};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio::time::timeout;

/// AI 客户端错误
#[derive(Debug)]
pub enum AiError {
    /// URL 无法解析，或协议不是 http/https
    InvalidUrl(String),
    /// 目标主机不安全（localhost/环回/私有/保留地址，或域名解析失败）
    UnsafeHost(String),
    /// 网络 / HTTP 层错误
    Http(reqwest::Error),
    /// 响应 JSON 解析失败
    Json(serde_json::Error),
    /// 协议层异常（非 2xx 响应、SSE 帧损坏等）
    Protocol(String),
    /// 请求超时
    Timeout,
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::InvalidUrl(m) => write!(f, "无效的 URL（仅允许 http/https）：{m}"),
            AiError::UnsafeHost(m) => write!(f, "拒绝访问不安全主机：{m}"),
            AiError::Http(e) => write!(f, "网络错误：{e}"),
            AiError::Json(e) => write!(f, "JSON 解析失败：{e}"),
            AiError::Protocol(m) => write!(f, "协议异常：{m}"),
            AiError::Timeout => write!(f, "请求超时"),
        }
    }
}

impl std::error::Error for AiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AiError::Http(e) => Some(e),
            AiError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for AiError {
    fn from(e: reqwest::Error) -> Self {
        AiError::Http(e)
    }
}

impl From<serde_json::Error> for AiError {
    fn from(e: serde_json::Error) -> Self {
        AiError::Json(e)
    }
}

/// 判断 IP 是否落在禁止访问的网段（环回/私有/链路本地/保留/组播等）
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // 0/8 未指定、10/8 私有、100.64/10 运营商级 NAT 保留、127/8 环回、
            // 169.254/16 链路本地（含云元数据服务）、172.16/12 私有、
            // 192.0.0/24 与 192.0.2/24 保留、192.168/16 私有、198.18/15 基准测试、
            // 198.51.100/24 与 203.0.113/24 文档保留、224/4 组播、240/4 与广播保留
            o[0] == 0
                || o[0] == 10
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || o[0] == 127
                || (o[0] == 169 && o[1] == 254)
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || o[0] >= 224
        }
        IpAddr::V6(v6) => {
            // IPv4 映射地址（::ffff:0:0/96）按内嵌 IPv4 判定
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_forbidden_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            // NAT64 64:ff9b::/96 内嵌的 IPv4 同样判定
            if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 {
                let v4 = Ipv4Addr::new(
                    (s[5] >> 8) as u8,
                    s[5] as u8,
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                );
                return is_forbidden_ip(IpAddr::V4(v4));
            }
            v6.is_unspecified()                       // ::
                || v6.is_loopback()                   // ::1
                || (s[0] & 0xfe00) == 0xfc00          // fc00::/7 唯一本地（ULA）
                || (s[0] & 0xffc0) == 0xfe80          // fe80::/10 链路本地
                || (s[0] & 0xff00) == 0xff00          // ff00::/8 组播
                || (s[0] == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 文档保留
        }
    }
}

fn check_ip(ip: IpAddr) -> Result<(), AiError> {
    if is_forbidden_ip(ip) {
        Err(AiError::UnsafeHost(format!(
            "{ip} 属于环回/私有/保留网段"
        )))
    } else {
        Ok(())
    }
}

/// 校验 base_url：
/// 1. 必须是合法 URL 且协议为 http/https；
/// 2. 主机名拒绝 localhost 及其子域；
/// 3. 字面量 IP 直接按网段判定（url 库会把 0x7f000001 等奇异写法规范化为 IP）；
/// 4. 域名则现场解析 DNS，任一解析结果命中禁止网段即拒绝（fail-closed）。
pub fn validate_url(raw: &str) -> Result<Url, AiError> {
    let url = Url::parse(raw).map_err(|e| AiError::InvalidUrl(format!("{raw}：{e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(AiError::InvalidUrl(format!(
                "协议 {other} 不受支持，仅允许 http/https"
            )))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| AiError::InvalidUrl(format!("{raw}：缺少主机名")))?;
    let bare = host.trim_end_matches('.').to_ascii_lowercase();

    if bare == "localhost" || bare.ends_with(".localhost") || bare == "localhost.localdomain" {
        return Err(AiError::UnsafeHost(format!("{bare} 是本地主机名")));
    }
    // IPv6 字面量在 host_str 中带方括号
    if bare.starts_with('[') && bare.ends_with(']') {
        let inner = &bare[1..bare.len() - 1];
        let addr: std::net::Ipv6Addr = inner
            .parse()
            .map_err(|_| AiError::UnsafeHost(format!("无法解析 IPv6 主机 {inner}")))?;
        check_ip(IpAddr::V6(addr))?;
        return Ok(url);
    }
    if let Ok(ip) = bare.parse::<IpAddr>() {
        check_ip(ip)?;
        return Ok(url);
    }
    // 域名：解析后逐个地址检查
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = (bare.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| AiError::UnsafeHost(format!("域名 {bare} 解析失败：{e}")))?;
    let mut any = false;
    for addr in addrs {
        any = true;
        check_ip(addr.ip())?;
    }
    if any {
        Ok(url)
    } else {
        Err(AiError::UnsafeHost(format!(
            "域名 {bare} 未解析到任何地址"
        )))
    }
}

/// 一条聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }
}

/// 角色铸造结果：AI 从设定文本中提炼的身份与初始特质
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgedCharacter {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub faction: String,
    #[serde(default)]
    pub traits: Vec<TraitSeed>,
}

/// 特质种子（铸造产物的组成项）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraitSeed {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// 记忆提炼结果：一段场景对话 → 摘要 + 关键词 + 印记深刻度
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefinedMemory {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 印记深刻度 0.0 ~ 1.0（越深刻越容易进入更高层记忆）
    #[serde(default)]
    pub salience: f32,
}

/// 「角色铸造」的消息序列（纯函数，便于测试）
pub fn forge_messages(setting_text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(
            "你是角色铸造师。阅读用户给定的角色设定文本，提炼出一个 JSON 对象，字段：\
             name（姓名）、title（称号）、origin（出身）、faction（阵营）、\
             traits（特质数组，每项含 name 与 description）。\
             只输出 JSON 对象本身，不要输出任何解释或 Markdown。",
        ),
        ChatMessage::user(setting_text),
    ]
}

/// 「记忆提炼」的消息序列（纯函数，便于测试）
pub fn refine_messages(scene_text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(
            "你是记忆提炼师。把用户给定的一段场景对话浓缩成一个 JSON 对象，字段：\
             summary（一两句摘要）、keywords（3~8 个关键词）、\
             salience（印记深刻度，0.0~1.0 的小数）。\
             只输出 JSON 对象本身，不要输出任何解释或 Markdown。",
        ),
        ChatMessage::user(scene_text),
    ]
}

/// 聊天补全响应（非流式），仅解析需要的字段
#[derive(Debug, Deserialize)]
struct ChatCompletion {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: Option<ChatRespMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatRespMessage {
    #[serde(default)]
    content: Option<String>,
}

/// OpenAI 兼容聊天客户端
pub struct OpenAiClient {
    http: Client,
    base_url: Url,
    api_key: String,
    model: String,
    request_timeout: Duration,
}

impl OpenAiClient {
    /// 创建客户端；`base_url` 形如 `https://host/v1`，构造即校验安全性
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Result<Self, AiError> {
        let url = validate_url(base_url)?;
        let http = Client::builder()
            // 禁止跟随重定向：公网地址可能 302 跳内网，跟随即绕过 host 校验
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base_url: url,
            api_key: api_key.to_string(),
            model: model.to_string(),
            request_timeout: Duration::from_secs(120),
        })
    }

    /// 自定义非流式请求的超时（流式请求只受连接超时约束）
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn build_body(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        json_mode: bool,
    ) -> serde_json::Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": stream,
        });
        if json_mode {
            // OpenAI 兼容的结构化输出开关；个别网关不支持时可去掉
            body["response_format"] = json!({ "type": "json_object" });
        }
        body
    }

    /// 发送聊天补全请求的统一入口：请求前重新校验 host（含 DNS），再发出请求
    async fn send_chat(
        &self,
        messages: &[ChatMessage],
        stream: bool,
        json_mode: bool,
    ) -> Result<reqwest::Response, AiError> {
        // 请求前校验：DNS 可能随时间变化，因此每次请求都重新解析判定
        let base = validate_url(self.base_url.as_str())?;
        let endpoint = base.join("chat/completions").map_err(|e| {
            AiError::InvalidUrl(format!("拼接 chat/completions 失败：{e}"))
        })?;
        let body = self.build_body(messages, stream, json_mode);
        let send = self
            .http
            .post(endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();
        let resp = timeout(self.request_timeout, send)
            .await
            .map_err(|_| AiError::Timeout)??;
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            if status.is_redirection() {
                return Err(AiError::Protocol(format!(
                    "上游返回 HTTP {status}：为防 SSRF 不跟随重定向（Location 已忽略）。{}",
                    truncate(&detail, 300)
                )));
            }
            return Err(AiError::Protocol(format!(
                "上游返回 HTTP {status}：{}",
                truncate(&detail, 300)
            )));
        }
        Ok(resp)
    }

    /// 非流式聊天：返回首个 choice 的内容
    pub async fn chat(&self, messages: &[ChatMessage]) -> Result<String, AiError> {
        let resp = self.send_chat(messages, false, false).await?;
        let parsed: ChatCompletion = resp.json().await?;
        parsed
            .choices
            .into_iter()
            .find_map(|c| c.message.and_then(|m| m.content))
            .ok_or_else(|| AiError::Protocol("补全结果为空".to_string()))
    }

    /// 流式聊天：返回 SSE 增量内容流（已跳过注释帧 / 空 delta / 终止帧）
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, AiError>> + Send>>, AiError> {
        let resp = self.send_chat(messages, true, false).await?;
        Ok(Box::pin(sse_content_stream(resp)))
    }

    /// 结构化 JSON 输出：请求 json_object 模式并解析为 T（带宽松兜底提取）
    pub async fn chat_json<T: DeserializeOwned>(
        &self,
        messages: &[ChatMessage],
    ) -> Result<T, AiError> {
        let resp = self.send_chat(messages, false, true).await?;
        let text = resp.text().await?;
        parse_llm_json(&text)
    }

    /// 角色铸造：从设定文本提炼 [`ForgedCharacter`]
    pub async fn forge_character(&self, setting_text: &str) -> Result<ForgedCharacter, AiError> {
        self.chat_json(&forge_messages(setting_text)).await
    }

    /// 记忆提炼：把一段场景对话浓缩为 [`RefinedMemory`]
    pub async fn refine_memory(&self, scene_text: &str) -> Result<RefinedMemory, AiError> {
        self.chat_json(&refine_messages(scene_text)).await
    }
}

/// SSE 帧解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// 增量文本内容
    Content(String),
    /// 流终止（data: [DONE]）
    Done,
    /// 其它可忽略帧（注释、event 帧、finish 帧等）
    Other,
}

/// 增量帧的 JSON 形态：{"choices":[{"delta":{"content":"…"}}]}
#[derive(Debug, Deserialize)]
struct DeltaFrame {
    #[serde(default)]
    choices: Vec<DeltaChoice>,
}

#[derive(Debug, Deserialize)]
struct DeltaChoice {
    #[serde(default)]
    delta: Option<DeltaPayload>,
}

#[derive(Debug, Deserialize)]
struct DeltaPayload {
    #[serde(default)]
    content: Option<String>,
}

/// 极简 SSE 解析器：跨 chunk 缓冲半行，逐行提取 data 帧
#[derive(Debug, Default)]
pub struct SseParser {
    /// 尚不构成完整行的尾部缓冲
    buf: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段原始文本，返回其中所有完整行解析出的事件
    pub fn feed(&mut self, text: &str) -> Vec<SseEvent> {
        self.buf.push_str(text);
        let buf = std::mem::take(&mut self.buf);
        let mut events = Vec::new();
        let mut consumed = 0usize;
        for (idx, ch) in buf.char_indices() {
            if ch == '\n' {
                let line = &buf[consumed..idx];
                consumed = idx + 1;
                if let Some(event) = parse_sse_line(line) {
                    events.push(event);
                }
            }
        }
        // 剩余半行留待下个 chunk
        self.buf = buf[consumed..].to_string();
        events
    }
}

/// 解析一行 SSE；返回 None 表示该行不产生事件（空行）
fn parse_sse_line(line: &str) -> Option<SseEvent> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() || line.starts_with(':') {
        // 空行（帧分隔）或注释/心跳
        return None;
    }
    let (field, value) = match line.split_once(':') {
        Some((f, v)) => (f, v.trim_start()), // SSE 规范允许冒号后有一个可选空格
        None => (line, ""),
    };
    if field != "data" {
        return Some(SseEvent::Other);
    }
    if value.trim() == "[DONE]" {
        return Some(SseEvent::Done);
    }
    match serde_json::from_str::<DeltaFrame>(value) {
        Ok(frame) => match frame
            .choices
            .into_iter()
            .find_map(|c| c.delta.and_then(|d| d.content))
        {
            Some(content) => Some(SseEvent::Content(content)),
            None => Some(SseEvent::Other),
        },
        Err(_) => Some(SseEvent::Other),
    }
}

/// 把响应体转换为「增量内容」流：内部逐 chunk 喂给 SSE 解析器
fn sse_content_stream(resp: reqwest::Response) -> impl Stream<Item = Result<String, AiError>> {
    stream::unfold(
        (
            resp,
            SseParser::new(),
            VecDeque::<Result<String, AiError>>::new(),
        ),
        |(mut resp, mut parser, mut pending)| async move {
            loop {
                // 先吐出上一轮缓存的事件
                if let Some(item) = pending.pop_front() {
                    return Some((item, (resp, parser, pending)));
                }
                match resp.chunk().await {
                    Ok(Some(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        for event in parser.feed(&text) {
                            if let SseEvent::Content(content) = event {
                                pending.push_back(Ok(content));
                            }
                        }
                    }
                    Ok(None) => return None, // 流正常结束
                    Err(e) => {
                        return Some((Err(AiError::Http(e)), (resp, parser, pending)))
                    }
                }
            }
        },
    )
}

/// 宽松解析模型输出中的 JSON：
/// 依次尝试 ①直接解析 ②剥离 Markdown 代码围栏 ③截取首个 `{` 到末个 `}` 的片段。
pub fn parse_llm_json<T: DeserializeOwned>(raw: &str) -> Result<T, AiError> {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }
    if let Some(body) = strip_code_fence(trimmed) {
        if let Ok(value) = serde_json::from_str(body.trim()) {
            return Ok(value);
        }
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            if let Ok(value) = serde_json::from_str(&trimmed[start..=end]) {
                return Ok(value);
            }
        }
    }
    Err(AiError::Protocol(format!(
        "无法从模型输出中解析出 JSON：{}",
        truncate(trimmed, 200)
    )))
}

/// 剥离 ``` / ```json 代码围栏，返回围栏内的文本
fn strip_code_fence(text: &str) -> Option<&str> {
    let text = text.trim();
    let start = text.find("```")? + 3;
    let after = &text[start..];
    // 跳过语言标记行（如 ```json）
    let after = after.split_once('\n').map(|(_, rest)| rest).unwrap_or(after);
    let end = after.rfind("```")?;
    Some(&after[..end])
}

/// 按字符数截断，超长补省略号（用于错误信息，防刷屏）
fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max_chars).collect();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes() {
        for url in [
            "ftp://example.com/v1",
            "file:///etc/passwd",
            "gopher://example.com",
            "javascript:alert(1)",
        ] {
            assert!(
                matches!(validate_url(url), Err(AiError::InvalidUrl(_))),
                "应拒绝协议：{url}"
            );
        }
    }

    #[test]
    fn rejects_local_private_reserved_hosts() {
        let bad = [
            "http://localhost/v1",
            "http://LOCALHOST:8080/v1",
            "http://api.localhost/v1",
            "http://localhost.localdomain/v1",
            "http://127.0.0.1/v1",
            "http://127.255.255.255/v1",
            "http://0.0.0.0/v1",
            "http://10.0.0.5/v1",
            "http://100.64.0.1/v1",
            "http://172.16.0.1/v1",
            "http://172.31.255.255/v1",
            "http://192.168.1.10/v1",
            "http://192.0.2.1/v1",
            "http://169.254.169.254/v1",
            "http://198.18.0.1/v1",
            "http://203.0.113.9/v1",
            "http://224.0.0.1/v1",
            "http://255.255.255.255/v1",
            "http://[::1]/v1",
            "http://[::]/v1",
            "http://[::ffff:10.0.0.1]/v1",
            "http://[fe80::1]/v1",
            "http://[fd00::1]/v1",
            "http://[2001:db8::1]/v1",
            "http://[64:ff9b::127.0.0.1]/v1",
        ];
        for url in bad {
            assert!(
                matches!(validate_url(url), Err(AiError::UnsafeHost(_))),
                "应拒绝主机：{url}"
            );
        }
    }

    #[test]
    fn accepts_public_literal_hosts() {
        for url in [
            "https://8.8.8.8/v1",
            "http://172.32.0.1/v1", // 172.32 不在 172.16/12 私有段内
            "http://[::ffff:8.8.8.8]/v1",
            "http://93.184.216.34:9000/v1",
        ] {
            assert!(validate_url(url).is_ok(), "应放行公网字面量：{url}");
        }
    }

    #[test]
    fn sse_parser_extracts_incremental_content() {
        let mut parser = SseParser::new();
        let events = parser.feed("data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n");
        assert_eq!(events, vec![SseEvent::Content("你".to_string())]);
    }

    #[test]
    fn sse_parser_buffers_partial_lines_across_chunks() {
        let mut parser = SseParser::new();
        // 半行不产出事件，留在缓冲里
        assert!(parser.feed("data: {\"choices\":[{\"delta\":{").is_empty());
        let events = parser.feed("\"content\":\"好\"}}]}\n\n");
        assert_eq!(events, vec![SseEvent::Content("好".to_string())]);
    }

    #[test]
    fn sse_parser_handles_done_comments_and_crlf() {
        let mut parser = SseParser::new();
        assert!(parser.feed(": keepalive\n\n").is_empty());
        assert_eq!(parser.feed("data: [DONE]\r\n\r\n"), vec![SseEvent::Done]);
        // 空 delta / finish 帧归为 Other
        assert_eq!(
            parser.feed("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n"),
            vec![SseEvent::Other]
        );
        // 非 data 字段（如 event: ping）归为 Other
        assert_eq!(parser.feed("event: ping\n"), vec![SseEvent::Other]);
    }

    #[test]
    fn parse_llm_json_plain_fenced_and_dirty() {
        let plain: ForgedCharacter = parse_llm_json("{\"name\":\"玄铃\"}").unwrap();
        assert_eq!(plain.name, "玄铃");

        let fenced: RefinedMemory = parse_llm_json(
            "```json\n{\"summary\":\"星图收敛\",\"keywords\":[\"星图\"],\"salience\":0.9}\n```",
        )
        .unwrap();
        assert_eq!(fenced.summary, "星图收敛");
        assert_eq!(fenced.keywords, vec!["星图".to_string()]);

        let dirty: RefinedMemory =
            parse_llm_json("好的，提炼结果如下：\n{\"summary\":\"夜谈\"}\n以上。").unwrap();
        assert_eq!(dirty.summary, "夜谈");

        assert!(parse_llm_json::<RefinedMemory>("完全不是 JSON").is_err());
    }

    #[test]
    fn prompt_builders_ask_for_json() {
        let msgs = forge_messages("北境守夜人玄铃");
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].content.contains("JSON"));
        assert_eq!(msgs[1].content, "北境守夜人玄铃");

        let refine = refine_messages("一段对话");
        assert!(refine[0].content.contains("salience"));
    }

    #[test]
    fn client_construction_rejects_unsafe_base_url() {
        assert!(OpenAiClient::new("http://127.0.0.1:8000/v1", "sk-test", "gpt").is_err());
        // 公网字面量可构造（不发请求）
        let client = OpenAiClient::new("https://8.8.8.8/v1", "sk-test", "gpt").unwrap();
        assert_eq!(client.model(), "gpt");
    }
}

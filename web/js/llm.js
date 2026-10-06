/* ling · OpenAI 兼容客户端（浏览器直连，流式 SSE）
   约束：URL 仅接受 http/https（服务端约束的客户端镜像；本应用无服务端，SSRF 面不存在） */
const LLM = (() => {
  /* SSRF 约束：仅公网 http(s)。拒绝 localhost/环回/私有/保留地址。
     （本地推理模型属 Slint 桌面版场景，浏览器原型一律指向公网 API） */
  function assertPublicHost(u) {
    if (u.protocol !== 'https:' && u.protocol !== 'http:') throw new Error('仅允许 http/https');
    const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, '');
    if (!host || host === 'localhost' || host.endsWith('.local') || host.endsWith('.internal')) {
      throw new Error('禁止访问本地/内网地址');
    }
    if (host.includes(':')) throw new Error('禁止访问本地/内网地址');   // 裸 IPv6 一律拒绝
    if (/^\d{1,3}(\.\d{1,3}){3}$/.test(host)) {
      const [a, b] = host.split('.').map(Number);
      const private_ = a === 127 || a === 10 || a === 0 ||
        (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) ||
        (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127);
      if (private_) throw new Error('禁止访问本地/内网地址');
    }
  }

  function endpoint(baseUrl, path) {
    let u;
    try { u = new URL(baseUrl.trim()); } catch { throw new Error('BaseURL 格式无效'); }
    assertPublicHost(u);
    return u.origin + (u.pathname.replace(/\/+$/, '') || '') + path;
  }

  /* 供调用方入口处先校验（断污点链） */
  function guard(baseUrl) {
    let u;
    try { u = new URL(String(baseUrl).trim()); } catch { throw new Error('BaseURL 格式无效'); }
    assertPublicHost(u);
    return u.origin;
  }

  function headers(apiKey) {
    const h = { 'Content-Type': 'application/json' };
    if (apiKey.trim()) h['Authorization'] = 'Bearer ' + apiKey.trim();
    return h;
  }

  /* 流式对话：onDelta(text) 增量回调；返回完整文本。signal 支持停止 */
  async function chatStream({ baseUrl, apiKey, model, messages, signal, onDelta }) {
    const res = await fetch(endpoint(baseUrl, '/chat/completions'), {
      method: 'POST',
      headers: headers(apiKey),
      signal,
      body: JSON.stringify({ model, messages, stream: true }),
    });
    if (!res.ok) throw new Error(`API ${res.status}: ${(await res.text()).slice(0, 200)}`);
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = '', full = '';
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split('\n');
      buf = lines.pop();
      for (const line of lines) {
        const t = line.trim();
        if (!t.startsWith('data:')) continue;
        const payload = t.slice(5).trim();
        if (payload === '[DONE]') return full;
        try {
          const delta = JSON.parse(payload).choices?.[0]?.delta?.content;
          if (delta) { full += delta; onDelta?.(delta); }
        } catch { /* 忽略不完整行 */ }
      }
    }
    return full;
  }

  /* 非流式（对话/铸造/提炼/近况）。json=true 时申请 response_format；
     端点不支持(400)则去掉该字段降级重试一次（提示词里已含 JSON 约定，仍可稳定出 JSON） */
  async function complete({ baseUrl, apiKey, model, messages, signal, json }) {
    const body = { model, messages };
    if (json) body.response_format = { type: 'json_object' };
    const res = await fetch(endpoint(baseUrl, '/chat/completions'), {
      method: 'POST', headers: headers(apiKey), signal, body: JSON.stringify(body),
    });
    if (!res.ok && json && res.status === 400) return complete({ baseUrl, apiKey, model, messages, signal });
    if (!res.ok) throw new Error(`API ${res.status}: ${(await res.text()).slice(0, 200)}`);
    return (await res.json()).choices?.[0]?.message?.content ?? '';
  }

  /* 连通性测试 */
  async function testConnection({ baseUrl, apiKey, model }) {
    const res = await fetch(endpoint(baseUrl, '/models'), { headers: headers(apiKey) });
    if (!res.ok) throw new Error(`API ${res.status}`);
    return true;
  }

  /* 从回复中稳健抠 JSON（容忍代码围栏） */
  function extractJson(text) {
    const m = text.match(/```(?:json)?\s*([\s\S]*?)```/);
    return JSON.parse((m ? m[1] : text).trim());
  }

  return { chatStream, complete, testConnection, extractJson, guard };
})();

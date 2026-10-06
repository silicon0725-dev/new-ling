/* ling · TTS 语音合成（双协议自动适配，浏览器直连）
   1) 标准 /audio/speech（OpenAI 兼容）
   2) chat 形态：TTS 模型走 /chat/completions，assistant 台词 +
      modalities:['text','audio'] → choices[0].message.audio.data (base64)
      （小米 MiMo 等 gateways 的 TTS 形态；音色走 audio.voice）
   返回可播放的 blob URL；调用方负责播放与 URL.revokeObjectURL */
const TTS = (() => {
  function resolve({ baseUrl, apiKey }) {
    const s = Store.settings;
    let url = (baseUrl || s.ttsBaseUrl || s.baseUrl || '').trim();
    url = url.replace(/\/(chat\/completions|completions|embeddings|audio\/speech|models)$/i, '')
             .replace(/([^:])\/{2,}/g, '$1/')            /* 折叠双斜杠(//v1 → /v1) */
             .replace(/\/+$/, '');
    const key = (apiKey || s.ttsApiKey || s.apiKey || '').trim();
    if (!url.trim()) throw new Error('未配置 TTS BaseURL');
    LLM.guard(url);                                   /* 同一私网防护 */
    return { base: url, key };
  }

  async function speechEndpoint({ base, key, model, voice, text, signal }) {
    const res = await fetch(base + '/audio/speech', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(key ? { Authorization: 'Bearer ' + key } : {}) },
      signal,
      body: JSON.stringify({ model, input: text.slice(0, 4096), voice, response_format: 'mp3' }),
    });
    if (!res.ok) throw new Error(`TTS ${res.status}: ${(await res.text()).slice(0, 160)}`);
    return await res.blob();
  }

  async function chatEndpoint({ base, key, model, voice, text, signal }) {
    const res = await fetch(base + '/chat/completions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(key ? { Authorization: 'Bearer ' + key } : {}) },
      signal,
      body: JSON.stringify({ model, modalities: ['text', 'audio'],
        audio: { voice: voice || 'alloy', format: 'mp3' },
        messages: [{ role: 'assistant', content: text.slice(0, 4096) }] }),
    });
    if (!res.ok) throw new Error(`TTS ${res.status}: ${(await res.text()).slice(0, 160)}`);
    const j = await res.json();
    const b64 = j.choices?.[0]?.message?.audio?.data;
    if (!b64) throw new Error('chat 响应未含音频数据');
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new Blob([bytes], { type: 'audio/mpeg' });
  }

  /* 生成语音 → { url }；自动适配: 先试标准端点, 网络层失败/404/405 再试 chat 形态 */
  async function synth({ baseUrl, apiKey, text, signal }) {
    const s = Store.settings;
    const { base, key } = resolve({ baseUrl, apiKey });
    const model = (s.ttsModel || '').trim();
    if (!model) throw new Error('未配置 TTS 模型');
    const voice = (s.ttsVoice || 'alloy').trim();
    const opts = { base, key, model, voice, text, signal };
    let blob;
    try {
      blob = await speechEndpoint(opts);
    } catch (e) {
      const retriable = /Failed to fetch|NetworkError|404|405|501/i.test(e.message);
      if (!retriable) throw e;
      blob = await chatEndpoint(opts);                  /* 网关未暴露 speech → chat 形态 */
    }
    if (blob.size < 128) throw new Error('TTS 返回空音频');
    return { url: URL.createObjectURL(blob) };
  }
  return { synth };
})();

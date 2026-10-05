/* ling · 设定文件解析(零依赖)
   读取 md / txt / docx / zip(归档) 为纯文本:
   - ZIP/DOCX 用浏览器原生 DecompressionStream('deflate-raw') 解压
     (Chrome 103+; 本应用 corner-shape 需 139+, 必然可用)
   - DOCX = zip 中的 word/document.xml, 剥标签留段落
   - zip 归档递归抽取其中 .md/.txt/.markdown/.docx 成员 */
const FileKit = (() => {
  const inflateRaw = async (u8) => {
    const ds = new DecompressionStream('deflate-raw');
    const buf = await new Response(new Blob([u8]).stream().pipeThrough(ds)).arrayBuffer();
    return new Uint8Array(buf);
  };
  const td = new TextDecoder('utf-8', { fatal: false });

  /* ZIP: EOCD 定位中央目录 → 逐条目 {name, method, data} */
  async function unzip(u8) {
    let eocd = -1;
    for (let i = u8.length - 22; i >= Math.max(0, u8.length - 66000); i--) {
      if (u8[i] === 0x50 && u8[i + 1] === 0x4b && u8[i + 2] === 0x05 && u8[i + 3] === 0x06) { eocd = i; break; }
    }
    if (eocd < 0) throw new Error('不是有效的 ZIP 文件');
    const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    const count = dv.getUint16(eocd + 10, true);
    let p = dv.getUint32(eocd + 16, true);
    const out = [];
    for (let i = 0; i < count; i++) {
      if (dv.getUint32(p, true) !== 0x02014b50) break;
      const method = dv.getUint16(p + 10, true);
      const compSize = dv.getUint32(p + 20, true);
      const nameLen = dv.getUint16(p + 28, true), extraLen = dv.getUint16(p + 30, true), cmtLen = dv.getUint16(p + 32, true);
      const localOff = dv.getUint32(p + 42, true);
      const name = td.decode(u8.subarray(p + 46, p + 46 + nameLen));
      if (!name.endsWith('/')) {
        const lnLen = dv.getUint16(localOff + 26, true), lxLen = dv.getUint16(localOff + 28, true);
        const dataStart = localOff + 30 + lnLen + lxLen;
        const raw = u8.subarray(dataStart, dataStart + compSize);
        out.push({ name, data: method === 0 ? raw : await inflateRaw(raw) });
      }
      p += 46 + nameLen + extraLen + cmtLen;
    }
    return out;
  }

  /* DOCX → 纯文本(段落换行) */
  const xmlToText = (xml) => xml
    .replace(/<w:p[^>]*>/g, '\n').replace(/<\/w:p>/g, '')
    .replace(/<w:tab[^>]*\/>/g, '\t').replace(/<w:br[^>]*\/>/g, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"').replace(/&apos;/g, "'")
    .replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();

  async function docxToText(u8) {
    const entries = await unzip(u8);
    const doc = entries.find(e => e.name === 'word/document.xml');
    if (!doc) throw new Error('DOCX 中未找到 word/document.xml');
    return xmlToText(td.decode(doc.data));
  }

  const isDocx = n => /\.docx$/i.test(n);
  const isText = n => /\.(md|markdown|txt)$/i.test(n);

  /* 入口: File 列表 → [{name, text}]; zip 递归展开 */
  async function readSettingFiles(files) {
    const out = [];
    for (const f of files) {
      const buf = new Uint8Array(await f.arrayBuffer());
      if (isDocx(f.name)) out.push({ name: f.name, text: await docxToText(buf) });
      else if (/\.zip$/i.test(f.name)) {
        for (const e of await unzip(buf)) {
          if (isDocx(e.name)) out.push({ name: f.name + ' › ' + e.name, text: await docxToText(e.data) });
          else if (isText(e.name)) out.push({ name: f.name + ' › ' + e.name, text: td.decode(e.data) });
        }
      }
      else if (isText(f.name)) out.push({ name: f.name, text: td.decode(buf) });
      else out.push({ name: f.name, text: '', error: '不支持的类型' });
    }
    return out;
  }

  return { readSettingFiles, unzip, docxToText };
})();

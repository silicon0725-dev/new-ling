# ling · Web 工作台（HTML 原型）

前端设计迭代版：**先在 HTML 里把交互与视觉打磨定稿，再照此移植 Rust + Slint 版**（`../crates/ling-app`）。

## 运行

零构建。任选其一：

```bash
# 方式一：任意静态服务器（推荐，避免 file:// 的种种限制）
cd web && python -m http.server 8941
# 方式二：直接双击 index.html（大多数功能可用）
```

打开 http://127.0.0.1:8941 → 「设置」页配置 OpenAI 兼容 API（BaseURL / Key 可留空 / 模型）→ 「角色」页铸造角色 → 开聊。

## 结构

```
web/
├── index.html        # 壳 + 五页面（对话/角色/领地/星图/设置），哈希路由
├── css/tokens.css    # 设计令牌单一真源（色板/字号/间距/动效曲线与时长）
├── css/app.css       # 组件与页面样式（只消费令牌，禁裸色值）
└── js/
    ├── store.js      # localStorage 本地优先存储（角色/特质/会话/叙事弧/近况）
    ├── llm.js        # OpenAI 兼容客户端：SSE 流式 + 非流式 + 连通性测试
    ├── starmap.js    # Canvas 力导向成长星图（发光节点/流光连线/星尘/缩放平移）
    └── app.js        # 路由与页面逻辑、流式对话、铸造、提炼、导入导出
```

## 设计约定（移植 Slint 时必须保持）

- **无色彩纪律**：唯一彩色是危险红 `#D8555A`；成功/确认用银白 `#C9CCD2`
- **动效**：统一曲线 `cubic-bezier(0.2,0.7,0.2,1)`；UI 动效 ≤300ms；只动 transform/opacity；同屏活跃动画 ≤2；`prefers-reduced-motion` 降级（对齐 Emil Kowalski design-eng 原则）
- **贴底跟随**：上滚越 56px 解除 + 「回到底部」浮钮；流式中切换会话不丢流
- **横幅行动必须接线**（无死按钮）；错误 Toast 驻留 6s、普通 4s
- 数据形状与 ling-core 对齐（特质活跃/休眠、场景→事件→叙事弧的弧层聚合），便于双向移植

## 素材致谢

- 图标：[lucide](https://lucide.dev)（ISC License），内联于 `js/icons.js`
- 音效：[uisfx](https://github.com/pielovedev/uisfx) minimal 包（CC0 1.0），位于 `assets/sfx/`，顶栏喇叭可静音

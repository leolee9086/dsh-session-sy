# session-sy

把 agent 会话日志（`.jsonl.zstd`）转成思源 `.sy` 文档，也能转回来。DSH 是第一个适配器。

## 两条路径

| 路径 | 用途 | 产出 |
|---|---|---|
| 导入思源（默认） | 归档、检索、引用、当长期记忆 | 只保留 DSH 前端会显示的内容 |
| `--spin` | 逐字节还原 | 结构完全相同，额外把原始事件挂在 callout 属性上 |

两条路径共用同一段组装逻辑：结构一致、顺序一致，spin 只是多挂了属性。

## 判据

只认外部事实，不拿自己跟自己对账——自己写的读、自己写的写，两边互相印证只能说明两份实现一致。

| 层 | 判据 | 现状 |
|---|---|---|
| 格式 | 思源自己的 `dataparser.ParseJSON` | needFix: false |
| 结构 | 导入运行中的思源，比「写进去的块」与「索引里留下的块」 | 937 块对得上 |
| Markdown | Lute 生成的 652 条 CommonMark 用例，逐字节比树 | 652/652 |
| 往返 | `restore(spin(x))` 与原始事件逐条比 | 150 事件一致 |

Markdown 这一层为什么重要：正文是 markdown，`.sy` 里却是 AST，任何一处解析差异导入后都会变成结构差异。所以解析器按 Lute 的源码结构移植，用 Lute 自己生成的用例逐字节校验。

格式层由 `tools/sycheck`（Go）实现，调的就是内核 `filesys/tree.go` 用的那个函数：

```go
tree, needFix, err := dataparser.ParseJSON(data, engine.ParseOptions)
```

## 文档结构

正文就是对话，一条消息一个 callout（提示块）：

```
NodeDocument
  NodeCallout            CalloutType=user       CalloutTitle=用户     CalloutIcon=👤
    NodeParagraph        markdown 解析出来的块
    ...
  NodeCallout            CalloutType=assistant  CalloutTitle=助手     CalloutIcon=🤖
    NodeCodeBlock        工具调用：工具名 + 完整参数
  NodeCallout            CalloutType=tool       CalloutTitle=工具结果  CalloutIcon=🔧
    NodeCodeBlock        工具结果原文
  ...
```

角色映射：`user/message` 用户 👤、`assistant/message` 助手 🤖、`system/message` 系统 ⚙️、`developer/message` 开发者 🛠、`tool/result` 工具结果 🔧、读图片的子调用 🖼。

几处细节：

- 工具结果整段包成一个代码块，不交给 markdown 解析（原文可能是几百行 JSON，解析会被拆散）
- 思考过程渲染成引用块，内部仍是 markdown 结构
- 图片复制进思源 `assets/`，按 `NodeImage` 引用；落不到时留一句「图片未落地」说明，不静默吞掉
- 空消息补一个空段落（思源要求每个 callout 至少有一个子块）
- 行内 HTML 降级成文本：`dataparser` 会把 `NodeInlineHTML` 强制转成 HTML 块，那会改变结构

## 签名

每个块都带来源签名。签名放在块上而不是文档上：块以后会被搬走、被引用、被重排，离开本文档也要能说清从哪来。

| 键 | 含义 |
|---|---|
| `custom-import-source` | 来源（`dsh`） |
| `custom-import-archived` | 归档时刻（毫秒时间戳） |
| `custom-dsh-session` | 会话 id |
| `custom-dsh-preset` | 会话用的预设 |
| `custom-dsh-model` | 产出这块内容的模型（只有 LLM 输出才标） |

前三个是通用键，将来接别的来源也用这套；后两个是 DSH 专有，来源不是 `dsh` 时不会写。

## spin 怎么做到可逆

- 每个消息 callout 带 `custom-dsh-event`：该事件的原始 JSON（base64）
- 文档属性带 `custom-dsh-header`：日志第一行（会话头）
- 非消息事件不集中收拢，而是留在所属位置上：`custom-dsh-before` 表示「这些事件发生在这条消息之前」，`custom-dsh-after` 存尾部剩余。顺序由位置本身表达，不需要全局索引表

还原时不读正文，只读这些属性，按事件序号排序拼回原始序列。

## 命令

```
session-sy render <session.jsonl.zstd> <out.sy> [--spin] [--assets DIR]
session-sy restore <doc.sy> [<session.jsonl.zstd>]
session-sy conformance <fixtures.json> [section]
session-sy parse <markdown>
```

- `render`：会话 → `.sy`。`--assets` 指向思源的资源目录，图片会复制进去
- `restore`：从 spin 文档还原事件；给了原始会话就逐条比对并报告结果
- `conformance`：跑 CommonMark 等价测试
- `parse`：打印 markdown 解析出来的树（调试用）

## 目录

```
rust/session-sy/             唯一实现
  src/lib.rs                 库入口
  src/md.rs                  块级解析入口（405 行）
  src/md/start.rs            各类块的 Start 判定
  src/md/continue.rs         Continue 判定
  src/md/finalize.rs         Finalize 与链接引用定义
  src/md/html.rs             HTML 块判定
  src/md/render.rs           序列化成 Lute JSON
  src/md/util.rs             空白裁剪与 base64
  src/md/inline.rs           行内解析入口（322 行）
  src/md/inline/text.rs      文本、反斜杠、换行、行内代码
  src/md/inline/emphasis.rs  分隔符栈与文本合并
  src/md/inline/bracket.rs   方括号与链接组装
  src/md/inline/link.rs      标签、目标、标题扫描
  src/md/inline/html.rs      行内 HTML 与字符实体
  src/md/inline/autolink.rs  尖括号自动链接
  src/render.rs              会话 → .sy（255 行）
  src/render/blocks.rs       各类型块的组装
  src/render/stamp.rs        签名与块 id
  src/session.rs             读 jsonl.zstd（多帧 zstd 拼接）
  src/model.rs               块 id 派生
  src/main.rs                CLI
  tests/conformance.rs       652 条 CommonMark 等价测试
tools/sycheck/               Go，用思源的 dataparser 校验 .sy
tools/sycheck/mdprobe/       用 Lute 生成测试用例
scripts/                     思源验证与导入的辅助脚本（只用 Node 内置模块）
```

## 已知限制

- 编辑器里改完再存回 DSH 这条路没做，只做导入与 spin 还原。
- 只适配了 DSH 一种会话格式。签名键分了通用与专有，但 `Session` 还是 DSH 的形状。
- 图片要落进思源的 `assets/` 才显示；Rust 进程写不进工作空间之外的目录时，需要先落本地再同步过去。
- 归档时间戳每次渲染都不同，所以同一会话两次渲染的 `.sy` 不会逐字节相同——可逆性靠事件内容，不靠文件哈希。

## 从哪读起

- 改渲染流程：`src/render.rs` 的 `document`
- 改块组装：`src/render/blocks.rs` 的 `message_callout`
- 改 markdown 解析：`src/md.rs` 是入口，按 Lute 的结构分成 start / continue / finalize / inline
- 加校验：`tools/sycheck` 调的就是思源内核同一个解析函数

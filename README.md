# session-sy

把 agent 会话序列转换成思源 `.sy` 文档，并能转换回来。DSH 是第一个适配器。

```
agent 会话  →  adapter  →  Session（头 + 有序事件）  →  spin  →  .sy（一片或多片）
                                              ↑                        |
                                              └──────  spinBack  ←───────┘
```

**只有一套实现，用 Rust 写。** 早先的 TS 版本在 `legacy-ts/`（仓库没有版本控制，留档不删）。

## 判据只有思源和 DSH，不拿自己跟自己对账

自己写的读、自己写的写，两边互相印证只能说明「我的两份实现一致」——
我先前拿这种自洽当验证，是错的：当时生成的文档缺引用块标记、缺围栏结构，
思源导入后 5 条消息丢了 4 条、代码块降级成文本，而我的「验证」照样报 ok。

现在只认两条外部事实：

| 层 | 判据 | 当前证据 |
|---|---|---|
| 格式 | 思源自己的 `dataparser.ParseJSON` | 2943 个会话 / 4943 片全部接受，零拒绝、零 needFix |
| 结构 | **导入运行中的思源**，比「写进去的块」和「索引里留下的块」 | 18 事件：callout 5/5、段落 34/34、代码块 3/3、标题 1/1；33509 事件 47 篇：callout 7970/7970、段落 263587/263587、代码块 3310/3310、文档 47/47 |
| 可编辑 | 在思源里 `updateBlock` 改段落，再 `getBlockKramdown` 读回 | 两次都读回编辑标记 |
| 往返 | `spinBack(spin(x))` 与 DSH 写出的 JSONL 逐字段深等 | 2943/2943 会话 — 只说明投影可逆，**不说明思源能用** |
格式层由 `tools/sycheck`（Go）实现，调的就是内核 `filesys/tree.go` 用的那个函数：

```go
tree, needFix, err := dataparser.ParseJSON(data, engine.ParseOptions)
```

`dataparser` 不在 Lute 里，是独立仓库 `github.com/siyuan-note/dataparser`。

## 思源会吃掉什么（实测）

`scripts/siyuan-probe-format.mjs` 造一份已知内容的 `.sy` 导入思源，再读块表：

| 写进去 | 思源留下 | 结论 |
|---|---|---|
| 引用块带 `NodeBlockquoteMarker` | 保留 | 必须写这个标记 |
| 引用块**不带**标记 | **整块丢弃** | 子节点还在也不会显示 |
| 代码块四段围栏（开/信息/正文/合） | 保留，块内代码围栏也保留 | 必须写全 |
| 只有 `IsFencedCodeBlock: true` 的代码块 | 降级成普通文本 | 不写围栏等于丢代码块 |

这两条是硬要求，所以 `model.rs` 里 `blockquote()` 一定塞标记、`code_block()` 一定写四段围栏。

## 文档结构

正文是对话：一条消息一个 **callout**（提示块），消息正文按换行切成**独立段落块**。
机器数据全部在文末附录。

```
NodeDocument              title = 会话标题
  NodeCallout             CalloutType=user  CalloutTitle=用户  CalloutIcon=👤
                          custom-dsh-index=8  custom-dsh-role=user
    NodeParagraph         "你的任务:..."
    NodeParagraph         "请执行:"            custom-dsh-gap=2
    NodeParagraph         "1. 在工具列表中..."  custom-dsh-gap=1
  NodeCallout             CalloutType=assistant  CalloutTitle=助手  CalloutIcon=🤖
    NodeParagraph         "I will call the tool"
    NodeCodeBlock         工具参数（custom-dsh-block-type=tool-call）
  NodeCallout             CalloutType=tool  CalloutTitle=工具结果  CalloutIcon=🔧
    NodeCodeBlock         工具结果
  ...
  NodeHeading             事件流（机器可读，勿手改）
  NodeCodeBlock           custom-dsh-kind=stream   ← 一行一个附录条目
```

角色 → callout：`user` 用户 👤、`assistant` 助手 🤖、`system` 系统 ⚙️、`developer` 开发者 🛠️、`tool` 工具结果 🔧。

**为什么按换行切段落**：整段输出塞进一个段落时，`.sy` 里就是一颗 `NodeText` 带一串软换行，
编辑器里看起来是「一大坨纯文本」，没法逐段改。切成独立段落块之后，每一行都能单独选中、编辑、移动。

**切了怎么还原**：段落块带 `custom-dsh-gap`（这行前面有几个换行）与 `custom-dsh-trail`（末尾有几个换行），
反向按 gap 拼回去，文本逐字节还原。同一事件的多个段落靠 `custom-dsh-block`（content 下标）连续分组。

附录条目三种：`{"h":会话头}`、`{"i":序号,"e":非可见面事件}`、`{"i":序号,"r":消息剩下字段}`。
反向按 `custom-dsh-index` 归一，不依赖块的摆放顺序，也不依赖 `seq` 是否连续。
## 分片

最大的一份会话解压后 539MB / 22 万事件，思源打不开，所以一个会话输出一片或多片：

- 默认单片上限 4MB，按块大小切；**一个事件的正文块永远落在同一片**
- 每片带 `custom-dsh-part` / `custom-dsh-parts`，`spinBack` 按序号拼回
- 33509 事件 → 34 片；539MB 那份 → 161 片

## 类型从哪来

不手写、不抽样。`scripts/generate-schema.mjs` 从 DSH 发布的 `docs/persistence-schema.json`
生成 `data/session-schema.json`（编译进二进制）：62 个事件、5 个可见面事件、7 种 content 块。

schema 也不完整：`tool-result` 是运行时才出现的扩展块类型，所以映射对未知类型必须有兜底。

## 命令

```
session-sy emit <session.jsonl.zstd> <outdir> [--max-bytes N]
session-sy verify <session.jsonl.zstd> <part.sy>...
session-sy roundtrip <session.jsonl.zstd> [--max-bytes N]
session-sy batch <root> [--max-bytes N] [--limit N]

node scripts/verify-siyuan.mjs --sample 40 | --all     导出并逐片交思源解析器
node scripts/siyuan-import.mjs <session> <workspace> <port> [--keep]   导入思源并比对块
node scripts/siyuan-probe-format.mjs <workspace> <port>                探思源保留哪些结构
```

思源内核端口是启动时挑的，从进程的监听端口取；token 从 `<workspace>/conf/conf.json` 读。

## 没有验证到的

- **全量导入思源只做了两个会话**（18 事件、33509 事件）。2943 个会话只过了 `dataparser`，
  没有逐篇导入；导入路径和 `dataparser` 不是同一段代码。
- **编辑后重建没做完。** 现在只有在附录缺失时用块上的 `seq` 兜底，还没有 `seq` 重编号、
  也没有拿 DSH 的 `Session.create()` 校验重建结果。
- **多适配器没做。** 属性前缀按适配器分开（`custom-dsh-*`），但 `Session` 还是 DSH 的形状。
- **块数放大。** 按换行切段落后，33509 事件变成 263587 个段落块（约 8 倍）。
  可编辑性换来的就是这个：思源要索引 26 万个块。整份全量跑一遍会更重，没有实测。
- **附录占体积。** 附录里 `assistant/message` 的 `stream` 重放数据最大，这是无损的代价。
- **软换行只在段落内部。** 段落内不再出现换行（每行一段），但若原文在行内用 `\n` 表达别的语义，
  已经被拆成两段，还原时靠 gap 拼回，可读性上仍是两段。

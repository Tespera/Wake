---
name: Wake
description: 本地 Coding Agent 会话资料库的 macOS 桌面设计系统
---

# Wake Design System

> 代码是唯一真相：`crates/wake/src/theme.rs` 定义主题，`workbench.rs` 定义界面结构与对话正文渲染（原 detail.rs 已并入）。修改视觉后同步本文。

## 设计命题

Wake 的目标不是展示技术感，而是让用户在几秒内重新找到并继续一段对话。界面采用 macOS 原生资料库心智，以稳定三栏承载范围、会话和正文；全文搜索作为贯穿整个工作台的命令级能力。

视觉遵循现代 macOS Liquid Glass 的层级原则：

- 先用稳定的窗口拖拽区、来源列表、工具栏、菜单和键盘路径建立结构。
- 用自适应材质色差表达层级，避免给每个区域加边框。
- 常驻界面不使用投影；阴影只留给搜索面板、菜单、确认框和通知。
- 系统蓝只表达主操作、选择和焦点；Agent 身份只由品牌图标表达（`AgentId::brand_icon(dark)`，侧栏 18px、列表/搜索/详情 15px；单色素材按深浅模式取白版或 `-light` 深墨版）。
- 详情阅读面是内容主角，应用框架主动退后。

浅色是银白和暖灰，深色是石墨和暖黑。禁止滑向黑底霓虹、终端面板或仪表盘卡片墙。

## 信息架构

窗口使用稳定的三栏选择模型，不做 iOS 式逐页推进：

1. **资料库侧栏**：全部会话、收藏、智能体和项目。
2. **会话流**：当前范围的会话，按更新时间扫读和筛选。
3. **阅读区**：当前会话的身份、操作、完整元信息和正文。

选择状态显式且稳定。收藏、置顶、导出、显示原文件和删除都围绕当前会话发生；设置使用独立场景，不作为侧栏目的地。

## 窗口与布局

| 项目 | 规格 |
|---|---|
| 默认窗口 | 按屏幕取 78% × 82%,钳进 [940×620, 1400×900] 后居中。上限是因为正文列宽固定 612,窗口再宽多出来的全是空白 |
| 最小窗口 | 940 × 620 |
| 窗口顶部 | macOS 主窗口使用 44px 透明标题栏，交通灯与拖拽区收在侧栏顶部；Windows 用原生标题栏，Linux 视 compositor 装饰协商而定 |
| 资料库侧栏 | 224px 固定宽度 |
| 会话流 | 336px 固定宽度 |
| 阅读区 | 剩余宽度，`min_w(0)`；正文最大宽度 720px |

窗口不绘制全宽标题栏。侧栏承接交通灯、窗口拖拽区和唯一的全文搜索入口；会话流与阅读区直接延伸到窗口顶部。侧栏、会话流、详情头和正文分别使用 `sidebar`、`list`、`background`、`popover` 材质表达层级，不加投影。

## 颜色

所有颜色必须来自 `theme.rs` 的语义 token(含 STAR_YELLOW 常量,以及对话区专用的 `bubble_bg` / `panel_bg` / `panel_border` / `inline_code_bg` 四个随模式取值的函数);其他 UI 文件禁止颜色字面量。Agent 品牌资产只能来自 `AgentId::brand_icon(dark)`(内嵌 PNG 路径,定义在 `wake-core/src/models.rs`),加新 agent 时一处改完。

### 主要材质

| token | 浅色 | 深色 | 用途 |
|---|---:|---:|---|
| `title_bar` | `#EDEDEA` | `#1B1B1A` | 侧栏顶部窗口拖拽区 |
| `sidebar` | `#EDEDEA` | `#1B1B1A` | 资料库侧栏 |
| `list` | `#F7F7F5` | `#20201F` | 会话流 |
| `background` | `#F1F1EF` | `#242422` | 阅读区外层 |
| `popover` | `#FDFDFC` | `#2C2C2A` | 阅读面、对话框、菜单 |
| `muted` | `#E8E8E5` | `#323230` | 图标底、角标、静默面 |
| `secondary` | `#E8E8E5` | `#30302E` | 次级按钮、快捷键标签 |

### 文字与交互

| token | 浅色 | 深色 | 用途 |
|---|---:|---:|---|
| `foreground` | `#1D1D1F` | `#F0EFED` | 正文与标题 |
| `muted_foreground` | `#686761` | `#A9A8A2` | 元信息与说明 |
| `primary` | `#0A84FF` | `#4C8DFF` | 主操作、焦点、激活状态 |
| `list_hover` | `#EDEDEA` | `#2A2A28` | 会话 hover |
| `list_active` | `#E3EBF6` | `#303B4C` | 当前会话 |
| `sidebar_accent` | `#DEDEDA` | `#343432` | 当前资料库范围 |
| `danger` | `#E5484D` | `#FF6B60` | 删除与错误 |
| `success` | `#2F9E63` | `#56C789` | 刷新完成 |

主色 tint 必须有语义。不得为了“更活泼”给图标、面板或 Agent 名称随意上色。

## 字体与字阶(六档硬规范)

字体使用 `.AppleSystemUIFont`,等宽内容使用 Menlo。主题基准 14px。

**所有 UI 字号必须引用 `crates/wake/src/ui.rs` 的 FONT_* 常量**——禁止裸 `px` 数字,禁止 `text_sm` 等 rem 工具类(rem 被 Root 钉在 14px,`text_sm` 实渲 12.25px 这类幽灵值是层级失控的根源)。

| 档位 | 常量 | 值 | 字重 | 用途 |
|---|---|---|---|---|
| Title | `FONT_TITLE` | 22px | Semibold | 中栏上下文大标题 |
| Heading | `FONT_HEADING` | 16px | Semibold | 详情页会话标题、空态主标题 |
| Msg user | `FONT_MSG_USER` | 14px | Regular | 对话区用户气泡正文 |
| Msg body | `FONT_MSG_BODY` | 14.5px | Regular | 对话区助手正文 |
| Msg thinking | `FONT_MSG_THINKING` | 12.5px | Regular | thinking / 工具卡头行与折叠内容 |
| Msg mono | `FONT_MSG_MONO` | 11.75px | Mono | 工具卡的参数与输入输出 |
| Body | `FONT_BODY` | 14px | Regular(列表/搜索结果标题 Medium) | 导航行、**侧栏组头**、列表标题、按钮、输入、对话框正文 |
| Caption | `FONT_CAPTION` | 12px | Regular | 列表副行、元信息、占位、空态提示、路径 chip、侧栏子级行 |
| Label | `FONT_LABEL` | 11px | Regular | 计数、快捷键徽标、状态栏、会话行与详情头元信息 |

**颜色三级制**:`foreground`(主文字)/`muted_foreground`(全部辅助文字)/`primary`(强调与激活)。不引入第四种文字灰;不在 muted 色上叠 opacity。

**间距刻度(4px 网格)**:`SPACE_XS/SM/MD/LG/XL/XXL` = 4/8/12/16/20/24,定义在 `ui.rs`。新代码引用常量或显式 `px()`;**对齐敏感处禁用 rem 间距类**——`p_2p5` 实为 8.75px、`p_3` 实为 10.5px(rem=14 折算),均不在网格上,是对齐失真的来源。存量 rem 类已于 2026-08-24 全量迁移完毕,代码中不再允许出现 rem 间距类(`p_0` 例外,零无幽灵值)。

**侧栏中轴(x = 26.75)**:traffic lights 定位 (20,11),红灯实测直径 13.5px,中心即 **26.75**。侧栏所有行首元素的**视觉中心**压在这条竖线上,而非左缘对齐:

| 元素 | 常量 | 值 | 推导 |
|---|---|---:|---|
| 容器内边距 | `SIDEBAR_EDGE` | 10 | 行 hover/选中胶囊的左右留白 |
| 行首槽位 | `LEAD_BOX` | 18 | 最大前导元素(品牌图)尺寸,槽内**居中** |
| 行左内边距 | `LEAD_INSET` | 7.75 | 26.75 − 9(槽位半宽) − 10 |
| 组头左内边距 | `GROUP_HEAD_INSET` | 12.125 | 由首字母字形中心反推(Body 常规下 A 宽 8.25、P 宽 7.25,两者中心恰好重合) |
| 标题左内边距 | `TITLE_INSET` | 9 | 由 "Wake" 的 W 反推(Heading semibold,W 宽 14.25、左承距 0.5) |
| 分组项缩进 | `SUB_INDENT` | 12 | 分组项行首中心落在 38.75,表达从属 |

三条硬约束:

1. **中心对齐与左缘对齐是同一个自由度,只能满足一个。** 选了中心,18px 品牌图的左缘就落在 17.75,比红灯左缘还靠左 2.25——这是预期结果,不是错位。同理分组项一旦缩进就不再压轴。
2. **`GROUP_HEAD_INSET` / `TITLE_INSET` 不是间距,是从字形宽度反推的值**,组头或标题的字号、字重一改立即失效,必须重新实测字形再算(11px semibold 时需 13.25/13.0,Body 常规时变成 12.125)。
3. **2x 屏光栅化步长是 0.5px,别往小数点后继续调。** 实测 `GROUP_HEAD_INSET` 取 12.125 落位 −0.125,改成 12.25 反而跳到 +0.375——两者落进不同物理像素。全部元素落在 ±0.375 以内即为达标。

**行高两级**(侧栏纵向层级的来源):主导航 `ROW_HEIGHT` 32px + Body 14,分组展开项 `ROW_HEIGHT_SUB` 26px + Caption 12。圆角均 8px。

UI 语言英文;会话正文保持原语言。元信息分隔符固定为前后带空格的 ` · `。

## 组件规范

### 窗口顶部

macOS 不设置横跨三栏的自定义 header。主窗口透明标题栏高 44px，系统交通灯在其中垂直居中；其所在拖拽区与侧栏使用完全相同的材质和颜色。会话流与阅读区不为标题栏预留另一条色带。Windows 保留系统原生标题栏（贴靠布局与深色模式经 DWM 处理）；Linux 由 compositor 装饰协商决定，报 Client 时侧栏顶部挂自绘 TitleBar。

### 资料库侧栏

- 侧栏顶端按红绿灯、`Wake` 标题和搜索框的可见边界做光学对齐：标题容器上留 4px、下留 16px。窗口控制区和品牌行各高 44px，合计 88px。
- 顶部是唯一的全文搜索入口,文案 "Search sessions",右侧显示 `⌘K`;Search/All Sessions/Starred 固定不随滚动。
- 搜索行必须有防溢出结构:标签文字 `flex_1 + min_w_0 + truncate`,图标与 `⌘K` 徽标显式 `flex_shrink_0`。裸文字子元素的最小宽度被内容锁死,侧栏一窄就会把右侧元素挤出边界裁掉。
- **行分两级**(侧栏纵向层级的来源,不得拉平):主导航 All Sessions/Starred 32px 行高 + Body 14;分组展开项(agent/项目)26px 行高 + Caption 12 + 整行右移 `SUB_INDENT` 12px 表达从属。
- **每行必须有行首元素**,由 `RowLead` 枚举强制(`Icon` 或 `Brand` 两态,无 `None`):主导航用 Lucide 单线图标,agent 行用品牌 PNG,项目行用 `folder.svg`。槽位定宽 `LEAD_BOX` 管右侧文字起点统一,槽内居中管中轴对齐。
- 线条图标比实心品牌图视觉轻,同档里给小一号:分组项 Lucide 14 / 品牌图 18,主导航 Lucide 15。
- 行内容 = 行首元素 + 标题 + 计数;计数一律 Label 档 muted。
- 组头 "Agents"/"Projects" 用 Body 档常规字重 + muted 色(与主导航同字号同字重,仅靠颜色和"无行首图标"区分——加粗会让组头压过它统辖的行),带 13px chevron 可折叠。
- 底部工具条常驻,总高 44px（含顶部 1px hairline）,按钮靠右排列(次要操作区:透明底、hover 才出色,不与导航行选中态抢注意力)——依次为 chart-column "Insights"、齿轮 "Settings"、refresh。Insights 页打开时其图标以 primary 点亮,是工具条里唯一有激活态的按钮。Settings 同时进入 Wake 菜单并绑定 `⌘,`(其他平台 `Ctrl+,`),保持单例窗口。
- Settings 默认 820×600，采用 180px 窄侧栏 + 内容页结构，固定为 General / Locations / Data / Updates / About 五项；About 与功能设置分离并钉在侧栏底部，Wake 菜单的 About Wake 直达同一页。About 沿用 Kooky/Birth 的信息顺序：产品图标、名称、版本、tagline、GitHub、短分隔线、版权/许可证与作者署名。Updates 是独立功能页,仅在用户点击页面按钮或 macOS Wake 菜单的 Check for Updates 时读取 GitHub 最新正式 Release 元数据,明确呈现检查中/最新版/有新版/失败四种状态;有新版时打开 Release 页供用户下载,不后台检查、不自行覆盖应用包。内部常规文字按钮统一沿用主界面的 24px 高、6px 圆角和主题交互色,普通页面动作使用 muted 填充 + hairline；发现新版后的 View Update 是需要用户继续完成的主操作,使用 32px 高 primary 填充和轻阴影。Appearance 分段选择器也使用同一材质。General 只放真实可用的全局偏好,当前为持久化的 System / Light / Dark 外观选择;不提供默认 “Open In” 终端。Data 只展示 Wake 本地存储位置、会话数与磁盘占用并提供文件管理器入口,不重复放刷新或清库动作；常规 Refresh 的唯一入口仍是主侧栏底部。Locations 页按 AgentId 声明序以 agent 分组,品牌名只在组头出现一次;本机有数据的组优先,未检测到的 agent 默认收进可展开区。每条路径以路径为主信息、会话数/不可用状态为 muted 副信息,最右为逐路径开关;停用时只降低文字层级，开关与菜单保持完整对比度。行本身不承担编辑,`…` 菜单集中 Edit / Show in Finder / 自定义 Remove。顶部操作为低强调的 Add location,Restore defaults 收进页级 `…` 菜单且无偏离时禁用。添加/编辑仍复用 agent 下拉 + 可手输路径 + 目录选择表单;关闭 location 后保留配置、停止扫描/监听并从会话与搜索结果排除,重新开启即增量扫回;纯路径管理不做内容校验。
- 工具条内的**状态行"常态沉默"**:仅刷新中或监听不可用时出现在按钮行上方;文案须可 truncate,窄侧栏放不下长句(故为 "Live updates off" 而非带操作建议的整句)。
- 手动 Refresh 始终后台运行；进度复用侧栏状态行，完成后发通知，不用模态框阻断浏览、搜索或阅读。
- 不把项目包装成卡片,不堆叠分支、时间或重复图标。项目行不加彩色标识——同一图标重复十几次不传递信息。

### 会话流

- 顶部由 22px 上下文标题与会话总数角标组成,右侧是 icon-only 排序按钮(当前排序方式由 tooltip 给出)。总数取全库计数,与侧栏 All Sessions 同源。
- **按时间分组**:Today / Yesterday / Earlier this week / 月份,组头 Label 11 semibold muted。仅在按更新/创建时间**倒序**时分组——按消息数排序时时间不单调,分组会碎成一堆单元素组;正序时组头顺序会反着读。
- 会话行两行:第一行只有标题(Body 14 medium),第二行是元信息。**标题必须是行容器的直接子级**,理由见下方「文字截断」。
- 第二行固定为:品牌图标 14px → 项目名 badge → ` · ` → 消息数 → 弹性空隙 → 收藏/置顶 → 相对时间。
- **agent 品牌图标固定保留**,它是每行的身份锚点,不做"当前范围内 agent 唯一就省略"的优化。
- model 不进列表:336px 的列放不下"项目 + 消息数 + model + 时间",硬塞会把项目名挤成两三个字。model 在详情页元信息带里。
- 列表被 limit 截断时,底部用一条 hairline + Label 档说明"Showing the N most recent of M",不静默截断。
- 空态按状态分四种:扫描中(索引进度)、扫描失败(错误)、有筛选无结果、全库为空。**扫描期间不得报"没有匹配项"**——那时既没有筛选也没有查询。
- 当前行使用低饱和蓝材质，不额外描边。
- 会话流不重复提供全文搜索入口；列表内输入只筛选当前范围。

### 详情头部

**只有两层**(2026-08-26 改版,原为六层):

1. 22px 会话标题(**单行**,按可用宽度自截断 + 全文 tooltip)与操作区。
2. 单条元信息带:品牌图标 13px · Agent 名 · 项目名 · git 分支 · model · source 徽标 · 消息数 · token · 相对时间,全部 Label 档 muted,用 ` · ` 串起。

“在终端继续”是唯一主按钮。收藏、置顶保留为独立图标按钮；导出、Finder 和删除进入“更多”菜单。按钮圆角固定 6px，危险操作在菜单中用分组隔开并继续走确认框。

三处信息**收进 tooltip 或菜单,不再占位**:

- 项目完整路径 → 项目名的 tooltip(点击仍然 Reveal)。
- 精确到秒的 Created / Updated → 相对时间的 tooltip。
- 会话 JSONL 文件路径 → 整行删除。中段是 UUID,对人零价值;Reveal 入口在 `…` 菜单里已经有了。

两条隐藏规则:git 分支为 `HEAD` / `detached` / 空时整块不渲染(detached HEAD 下透传的 "HEAD" 零信息量);model 用同级 muted 文字而**不是** badge——原先那个 outline badge 用的是 Claude 橙,却被套在所有 agent 上,与"品牌色只表达 agent 身份"自相矛盾。

对话框标题一律 Heading 16 semibold:组件内建 `.title()` 不设字号(实渲窗口默认 14px),必须显式补 `text_size(FONT_HEADING)`。破坏性确认的主按钮点名动作并用 danger 形态("Move to Trash",Windows 上经 trash_copy! 平台文案为 "Move to Recycle Bin",不留裸 "OK");表单弹窗内控件同档取齐(输入框与下拉/浏览钮同高,次级动作行才允许 small)。

### 对话阅读面

正文位于 `popover` 材质的 12px 圆角阅读卡中，外层使用 `background`。渲染形制对标 Claude Desktop(2026-08-26 用户定)。

**角色由形态区分,不用文字标签**:

- 用户消息 = 右对齐气泡,`RADIUS_BUBBLE` 18px 圆角、`bubble_bg` 暖灰底、最大宽 `BUBBLE_MAX_W`、内边距 17×11。
- 助手消息 = 全宽平铺,无气泡无标签。
- `Context compacted` 与 System 消息 = 居中胶囊。

**三个数是一组,必须一起改**:正文 `FONT_MSG_BODY` 14.5px、行高 `LINE_HEIGHT_PROSE` 1.92、列宽 `PROSE_MAX_W` 612px ≈ 42 个中文字/行。只提字号会让每行字数更少,只放宽容器会让行更长,单调任何一个都比原状更糟。气泡收半档(14px / 1.8),因为它有底色,同字号会显得更重。

行高取 1.92 而不是西文常用的 1.5–1.6:汉字没有 x-height 起伏,字面率高,靠行距拉开视觉通道。段间距 `PROSE_PARAGRAPH_GAP` 1.05rem(=14.7px)比行距再大半档,段落边界才读得出来。

**字间距调不了**:gpui 0.2.2 的 `Styled` 没有 `letter_spacing`,中西文混排的补偿只能靠行距和行宽间接缓解。

**markdown 层级走组件内置能力**(`TextView` 已支持 table / blockquote / divider / 标题分级 / tree-sitter 高亮),UI 层只做样式覆写,不自己解析:

- 标题分级经 `heading_font_size`:h1 ×1.45 / h2 ×1.28 / h3 ×1.14 / h4 ×1.05,再往下与正文同号靠字重区分。对话里的标题不该有网页 h1 的体量(组件默认是近两倍),但也不能差得太小,否则标题会混进正文。
- **标题从 markdown 里切出来单独渲染**(`workbench::markdown_message` + `split_markdown_blocks`),上间距由 `heading_top_gap(level)` 按层级给:h1 30 / h2 25 / h3 20 / h4+ 16px。

  为什么必须切:组件的标题渲染只有 `pb(rems(0.3))`、**没有 `pt`**(`node.rs` 的 `Node::Heading` 分支),标题上方的间距完全来自前一个块的 `paragraph_gap`——和两个普通段落之间一模一样,所以标题读不出层级。调大 `paragraph_gap` **不解决问题**:它把段间距和标题上间距一起放大,相对关系不变。`TextViewStyle` 也没有对应钩子。

  代价:一条消息会拆成多个 `TextView` 实例,**跨块的文字选择会断在块边界**。切块只按 ATX 标题(`# ` ~ `###### `),并跟踪围栏代码块状态,代码里的 `# ` 注释不会被误判。
- 块间距 `PROSE_PARAGRAPH_GAP` 1.05rem(14.7px),只管段落 / 列表 / 代码块 / 表格之间。
- 代码块经 `TextViewStyle::code_block` 套 `panel_bg` + `panel_border` + `RADIUS_PANEL`;右上角经 `code_block_actions` 挂"语言名 + 复制"。组件把 actions 绝对定位在块内右上,做不了横跨顶部的工具栏,所以两者合成一个胶囊。
- **代码块跟随深浅切换有两个坑,两处都必须做**:
  1. `TextViewStyle.highlight_theme` 要显式按模式给 `default_dark()` / `default_light()`。默认值恒为 light;而且 `TextViewStyle` 的 `PartialEq` **只**比较 `paragraph_gap` / `heading_base_font_size` / `highlight_theme`——`is_dark` 与 `code_block` 都不参与,不换 highlight_theme 的话新旧 style 会被判定相等,组件根本不重建。
  2. `TextView` 的 **id 要带上深浅模式**。它只在首次 `request_layout` 时同步解析,之后 style 变化走异步更新通道,带 200ms debounce + 后台线程重新解析(`UpdateFuture::new(.., Duration::from_millis(200), ..)`);而语法高亮的颜色是在**解析阶段**固化进 `CodeBlock.styles` 的,于是切换主题后代码块要慢一拍才变色。id 带上模式后,主题一换就被当作新元素,走首次的同步解析路径。

**thinking 与工具调用都是可折叠面板**(`RADIUS_PANEL` 圆角 + `panel_bg` + `panel_border`),视觉上比正文退后一档:

- thinking 收起时头行给一句摘要,展开是完整原文。不得再做成"永久截断的一行"。
- 工具卡头行 = chevron + 名称 + **参数摘要** + 结果徽标。收起时就要看得出操作对象,只给工具名等于没有信息。
- 展开后逐条给 Input 与 Output。**成功调用的输出同样要显示**——只留失败项等于把绝大多数工具结果丢掉,而这是个"回看历史"的工具。
- 单条调用时头行已给出参数,展开体不再重复名称;多条时头行给数量与名字序列。

emoji 不承担界面或正文结构图标职责。

### 空态

详情空态是 360px 宽的阅读材质面：58px 图标圆面、Heading 16 主句和 Caption 12 说明，内边距 `SPACE_XXL`。

空态标题**陈述状态**，不喊口号("No session selected"，而非 "Find that conversation" 这类无指代对象的祈使句)；说明只留一句、给一个可执行动作并直接点名快捷键("Pick one from the list, or press ⌘K to search.")。空态不重复放搜索按钮。会话列表无结果同构("No matching sessions" + 清空筛选或更换条件)，尺寸更紧凑。

### 全文搜索

- 面板宽 680px，距窗口顶部 72px。
- 大尺寸无外框搜索输入置顶。
- 未输入与无结果状态高 250px；结果列表高 460px。
- 结果行使用品牌图标、标题、项目与时间、单行片段。
- 搜索始终覆盖全部会话；页脚左侧显示“搜索范围：全部会话”，右侧显示 `↑↓`、`↩`、`esc` 键盘路径。
- 指针回调中不得同步派发新的键盘事件。需要关闭旧浮层或转移输入焦点时，应通过对应组件 API 或延迟到下一事件周期处理，避免在 AppKit `mouseUp` 路径里重入 GPUI 事件分发；打开搜索面板前必须让 Root 先保存原焦点，关闭后 `⌘K` 才能继续生效。

## 文字截断(gpui 0.2.2 限制)

**`.truncate()` / `.text_ellipsis()` 在会话流与阅读区都不画省略号。**

gpui 在 `elements/text.rs:357` 只有拿到 `known_dimensions.width` 或 `AvailableSpace::Definite` 时才截断加 `…`;虚拟列表(`v_virtual_list`)行内与 flex 子项拿不到确定宽度,文字于是按 max-content 铺开,再被外层 `overflow_hidden` 硬裁——中文正好切在半个字上。

排查时试过四种结构,**都不出省略号**:标题带 `flex_1()`、去掉 `flex_1` 配弹性占位、把标题提成行容器的直接子级、去掉 `ListItem` 的 `mx`。

给文字元素显式 `.w()` **也不管用**——省略号照样不画。所以定宽区域的文字一律走 `format::clip_display(s, cells)` 按**显示宽度**自截断(CJK / 全角记 2 格,其余记 1 格),并配 `overflow_hidden() + whitespace_nowrap()` 兜底:

| 位置 | 格数来源 | 依据 |
|---|---|---|
| 会话行标题 | `TITLE_CELLS` = 42(常量) | 会话流列宽写死 336px,阈值稳定 |
| 会话行项目名 | `PROJECT_CELLS` = 14(常量) | 与消息数、时间共享第二行 |
| 详情页标题 | `cells_for(title_w, CELL_PX_TITLE)` | 窗口宽 − 侧栏 − 会话流 − 内边距 − 操作区 |
| 工具卡参数 | `cells_for(prose_w − 151, CELL_PX_MONO)` | 正文实际列宽(窄窗口下小于 `PROSE_MAX_W`) |

三条硬约束:

1. **宽度会变的地方,格数必须从当前像素宽反算,不能写死。** 阅读区宽度随窗口拖拽变化,写死格数会让"窗口拉宽后截断长度不补齐"——这是实打实的 bug,不是取舍。`cells_for()` 每帧从 `window.viewport_size()` 重算。
2. **省略号自己占两格。** `…` 是全角,`clip_display` 里给它留 2 格而不是 1 格;只留 1 格时截出来的串会超出容器,`…` 正好落在边界外被裁,看起来就是"截断了但没有省略号"。有单测钉住这条。
3. **列表行绝不允许换行。** `List` 只测量一行的高度然后套给所有行(`list.rs:406`),一行变两行会把后面每一行都挤错位。格数是估算值,所以 `whitespace_nowrap()` 必须留着。

上游哪天修好 `text_overflow`,连同 `clip_display` 的调用点一起删掉即可。

### Insights

侧栏底部工具条的 chart-column 按钮进入;它是与全部导航行互斥的**整页目的地**——打开时替换会话流与阅读区,点任意导航行(或再点入口)退出并落回 All Sessions。设置仍是独立场景,Insights 不是。

- 页面用 `background` 材质整片承载;顶部 88px 标题区与中栏同节奏(Insights 22px semibold + Label 11 副行,副行只说 "Since {首会话月份}"),兼窗口拖拽区。内容限 720px 阅读宽居中,区块之间只用 32px 留白与 Body 14 semibold 组头分隔——**不做卡片墙、零投影**,延续"避免每个区域加边框"的层级原则。
- 统计口径与主 UI 一致(archived 不计):"Prompts" 一律指主线用户消息。数据在打开与每次 Refresh 后后台重算,不阻塞浏览。
- 概览行:22px semibold 大数字 + Caption 标签,序为 Sessions / Tokens / Prompts / Agents / Projects / Active days(用户钉序,2026-08-27);Tokens 仅在有 agent 报过用量时出现。数字千分位。
- 活跃热力图:53 周 × 7 天(周一起始,最右列为本周),10px 圆角格 + 3px 缝,总宽 715px。强度 = `muted` 空格 + `primary` 25/50/75/100% 四档(按窗口内峰值分位);未来日期留白。月份与 Mon/Wed/Fri 标签用 Label 11 muted;每格 tooltip 给 "N prompts · Aug 3, 2026"。底注左侧为 streak 与最忙一日(Label 11,` · ` 分隔),右侧 Less–More 固定满阶梯图例。
- 分布图:hour(24 柱)/ weekday(7 柱)/ month(12 柱)三个维度共用一张竖柱图,组头右侧 ‹ › ghost 按钮循环切换(纯视图状态,数据三份常驻不重查);峰值柱全饱和 `primary`、其余 55%,零值保留 2px `muted` 基线;柱数越少缝越大(4/8/6px)。hour 只标 6 小时锚点(靠左),weekday/month 每柱标签与柱居中;组头副行点出峰值("Most active around 2 PM" / "on Sundays" / "in August")。
- Agents / Projects / Models 三个榜单同构:条形行 = 行首(品牌图 15px 原色 / folder 图标 / 无)+ 名称列定宽 truncate + 6px 圆头轨道条(`muted` 轨、`primary` 填充,按组内峰值归一)+ 右对齐 Label 计数。三个组头都挂 ‹ › 切换度量,循环序与概览行一致:Sessions / Tokens / Prompts;**当前档位名(首字母大写的裸名词)显示在两键中间**——64px 定宽居中,Caption muted,按钮位置不随文本跳动;榜单组头因此为单行(标题与按钮组居中对齐),分布图组头保留 caption 双行、按钮中间无标签(其标题本身就是档位名)。每个榜单各自记忆档位,行按当前度量降序重排后取 top-N(Agents 全量、Projects/Models 各 6;截断在排序之后,换度量不漏项);Tokens 档只列报过用量的组、值用 K/M 缩写,组内无人报 token 时该档不进循环。
- 空态沿用详情空态形制("No activity yet" + "Refresh sessions to see your activity here.");加载用居中 Spinner,已有数据时静默换新不闪烁。

## 图标、形状与层次

- UI chrome 只使用内嵌 Lucide 单线 SVG，不使用 Unicode 或 emoji 图标；Agent 身份用内嵌品牌 PNG，经 `img()` 渲染并**保持原色**(不得用 `text_color` 着色,选中态也不变色)。
- 品牌 PNG 登记在 `assets.rs` 的 `brands!` 宏,文件名 = `AgentId::as_str()`,路径含 `.png`。入库前须裁掉透明边并保持正方形；带白色/彩色底的 app-icon 风格图必须先抠底,否则在侧栏材质上会露出白方块。
- 品牌图标侧栏分组项 18px、内容区(列表/搜索/详情)15px；Lucide 行内图标 13–15px；主操作与工具栏图标 14–16px；空态图标 22–26px。
- 面板圆角 12px，列表与侧栏选择 8px，按钮固定 6px，快捷键标签 5px，badge 胶囊 4px。代码里前两档走 `theme.radius_lg` / `theme.radius`，后三档见 `ui.rs` 的 `RADIUS_BUTTON` / `RADIUS_KBD` / `RADIUS_BADGE`——不要再写裸数字。
- 常驻界面零投影、零渐变、无装饰性描边。菜单、命令面板、确认框和通知由组件库提供浮层阴影。
- 相邻的自定义材质只用一套 token 和圆角语言，避免每个按钮各自模拟玻璃。

## 可访问性与桌面交互

- 不依赖颜色单独传达 Agent 或状态；品牌点/品牌图标旁必须出现 Agent 名称。
- 所有主要操作必须同时有指针和键盘路径；全文搜索为 `⌘K`，全量刷新为 `⌘R`。
- 控件使用 tooltip；菜单项使用“动词 + 对象”的完整英文标签(如 "Refresh Sessions")。
- 双模式使用同一语义结构，只有 token 值变化。
- 最小窗口宽度必须保证标题、主操作和更多菜单不互相挤压。

## 实现守则

- 先改标准结构和控件，再添加自定义材质面。
- 颜色只改 `theme.rs`；图标必须登记到 `assets.rs`，路径包含后缀(`.svg` / `.png`,漏后缀 = 静默空白)。
- 所有交互元素先设置 `.id()` 再绑定点击或滚动行为。
- `Root::render_dialog_layer` 与 `Root::render_notification_layer` 必须保留。
- 对原始 Agent 数据目录继续只读；任何视觉改造不得破坏刷新、搜索跳转、恢复或删除语义。
- 术语统一:用户可见文案一律说 **Refresh** 与 **Session**,不出现 scan / rescan / rebuild / index(这些只保留在数据层内部命名中)。

## 验收

```bash
cargo build -p wake
scripts/build_and_run.sh --verify
```

视觉验收至少覆盖：空态、选中会话、详情阅读、更多菜单、`⌘K` 搜索，以及系统浅色和深色模式。

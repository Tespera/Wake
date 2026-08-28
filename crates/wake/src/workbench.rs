// ============================================================================
// DIRECTION CONTRACT (impeccable)
// THESIS: 找回任何一段 agent 对话只需几秒;界面以 macOS 原生语言隐入背景,
//   拒绝"开发者工具=黑底霓虹终端风"的品类默认。
// OWN-WORLD: Things/Bear + Claude 客户端基准的原生 macOS 质感——暖白/暖黑双模式、
//   色差分区(无 hairline 依赖)、8px 圆角胶囊选中态(按钮 6px)、系统蓝 accent、
//   lucide 单线图标、SF 系统字体 14px 基准;agent 品牌色仅作识别圆点。
// STORY: 打开即见全部会话按时间流动;左栏收窄范围,中栏定位会话,右栏读全文;
//   ⌘K 直达任意一句话;一键回到终端继续。
// FIRST VIEWPORT: 全高三栏——224px 侧栏(全局搜索/全部/收藏/智能体/项目)、
//   336px 会话列表(上下文标题+会话数量+双行列表)，余宽为详情阅读器。
// FORM: brief-pinned canon(用户指定"现代 macOS 设计规范",对标 Things/Bear);
//   concept tournament 依规跳过,canon at full fidelity。
// FINISH: unreviewed and undocumented is unfinished; this build ends with the
//   finish review, the verdict, and DESIGN.md
// ============================================================================
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::highlighter::HighlightTheme;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::notification::Notification;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::spinner::Spinner;
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::{
    h_flex, v_flex, ActiveTheme as _, Icon, IndexPath, Root, Sizable as _, StyledExt as _,
    TitleBar, WindowExt as _,
};

use wake_core::adapters::{
    adapter_for, create_adapter_roster_for, path_owns, AdapterLocation, AgentAdapter,
};
use wake_core::db::Store;
use wake_core::models::*;
use wake_core::scanner::{run_scan, ScanEvents, ScanProgress};
use wake_core::services::{exporter, terminal};
use wake_core::watcher::{start_watcher, SessionWatcher};

use crate::format::{
    abs_date, clip_display, expand_tilde, fmt_tokens, month_year, one_line, relative_time,
    thousands, tilde_path,
};
use crate::settings::{SettingsPage, SettingsView};
use crate::ui::*;
use crate::update::{self, UpdateStatus};

actions!(
    wake,
    [
        ToggleSearch,
        RefreshSessions,
        OpenSettings,
        OpenUpdates,
        OpenAbout,
        PaletteUp,
        PaletteDown
    ]
);

pub const KEY_CONTEXT: &str = "Workbench";
/// ⌘K 面板容器的 key context(main.rs 的 ↑↓ 绑定与 dialog 元素共用)
pub const PALETTE_CONTEXT: &str = "WakePalette";
/// ⌘K 面板内容总高(输入行 + 结果列表 + footer);列表 flex_1 吃剩余空间
const PALETTE_HEIGHT: Pixels = px(492.);
/// location 表单标签列宽(Agent/Folder 两行共用)
const FORM_LABEL_W: Pixels = px(52.);
/// Wake 主窗口的透明标题栏高度。28px 详情操作条上下各保留 8px。
const WINDOW_TITLEBAR_HEIGHT: Pixels = px(44.);
/// 左栏顶部由 44px 窗口控制区 + 44px 品牌行组成；中栏标题区共享总高度。
const LIBRARY_IDENTITY_HEIGHT: Pixels = px(88.);
/// 侧栏底部常态工具栏内容高；加上父容器 1px 顶部分隔线，总高 44px。
const SIDEBAR_FOOTER_ROW_HEIGHT: Pixels = px(43.);

type SharedAdapters = Arc<Vec<Box<dyn AgentAdapter>>>;
type SharedLocations = Arc<Vec<AdapterLocation>>;

fn icon(path: &'static str) -> Icon {
    Icon::empty().path(path)
}

/// 起一条后台扫描线程。启动时的自动扫描(full=false)与用户主动重扫(full=true)
/// 共用;返回的 Result 由 run_scan 的终态事件代为上报,这里只需丢弃。
fn spawn_scan(
    adapters: SharedAdapters,
    store: Arc<Store>,
    events: Arc<dyn ScanEvents>,
    full: bool,
) {
    std::thread::spawn(move || {
        let _ = run_scan(&adapters, &store, events.as_ref(), full);
    });
}

// ---------------- 后台事件桥 ----------------

enum BgEvent {
    Progress(ScanProgress),
    Changed,
}

struct ChannelEvents(futures::channel::mpsc::UnboundedSender<BgEvent>);

impl ScanEvents for ChannelEvents {
    fn on_progress(&self, p: &ScanProgress) {
        let _ = self.0.unbounded_send(BgEvent::Progress(p.clone()));
    }
    fn on_sessions_changed(&self) {
        let _ = self.0.unbounded_send(BgEvent::Changed);
    }
}

// ---------------- 会话列表 delegate ----------------

pub struct SessionsDelegate {
    pub sessions: Vec<SessionMeta>,
    /// 按时间分好的组:(组头文案, 在 `sessions` 里的下标区间)。
    /// 存区间而不是各存一份 SessionMeta——那是十几个 String 字段的深拷贝。
    /// 不分组时是单个组名为空的组,`render_section_header` 返回 None。
    groups: Vec<(SharedString, std::ops::Range<usize>)>,
}

impl SessionsDelegate {
    fn new(sessions: Vec<SessionMeta>, sort: SortKey, ascending: bool) -> Self {
        let groups = build_groups(&sessions, sort, ascending);
        Self { sessions, groups }
    }

    /// IndexPath → `sessions` 的扁平下标
    fn flat_index(&self, ix: IndexPath) -> Option<usize> {
        let (_, range) = self.groups.get(ix.section)?;
        let flat = range.start + ix.row;
        (flat < range.end).then_some(flat)
    }

    /// 扁平下标 → IndexPath(选中与滚动定位用)
    fn index_path(&self, flat: usize) -> Option<IndexPath> {
        let (section, (_, range)) = self
            .groups
            .iter()
            .enumerate()
            .find(|(_, (_, r))| r.contains(&flat))?;
        Some(IndexPath::new(flat - range.start).section(section))
    }
}

/// 把会话切成 Today / Yesterday / Earlier this week / … 的时间组。
///
/// 只在**时间倒序**下分组:按消息数排序时时间不单调,分组会碎成一堆单元素
/// 组;正序时组头顺序会反着读。其余情况退回单个匿名组(不渲染组头)。
fn build_groups(
    sessions: &[SessionMeta],
    sort: SortKey,
    ascending: bool,
) -> Vec<(SharedString, std::ops::Range<usize>)> {
    if sessions.is_empty() {
        return Vec::new();
    }
    let groupable = matches!(sort, SortKey::Updated | SortKey::Created) && !ascending;
    if !groupable {
        return vec![(SharedString::default(), 0..sessions.len())];
    }
    let key_of = |s: &SessionMeta| match sort {
        SortKey::Created => s.created_at,
        _ => s.updated_at,
    };
    let mut groups: Vec<(SharedString, std::ops::Range<usize>)> = Vec::new();
    // 置顶在 DB 层就排到了最前(ORDER BY pinned DESC),它们的时间戳与后面
    // 的行不连续。混进时间分组会切出重复组头——置顶里有今天的、后面还有
    // 今天的,就会出现两个 Today。单独成组
    let pinned = sessions.iter().take_while(|s| s.pinned).count();
    if pinned > 0 {
        groups.push((SharedString::new_static("Pinned"), 0..pinned));
    }
    for (ix, s) in sessions.iter().enumerate().skip(pinned) {
        let label = time_bucket(key_of(s));
        match groups.last_mut() {
            Some((last, range)) if *last == label => range.end = ix + 1,
            _ => groups.push((label, ix..ix + 1)),
        }
    }
    groups
}

/// 时间戳 → 组头文案。UI 语言英文,月份走 chrono 的英文月名。
fn time_bucket(ts: i64) -> SharedString {
    use chrono::{Datelike as _, Local, TimeZone as _};
    let Some(dt) = Local.timestamp_millis_opt(ts).single() else {
        return "Undated".into();
    };
    let today = Local::now().date_naive();
    let days = (today - dt.date_naive()).num_days();
    match days {
        i64::MIN..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => "Earlier this week".into(),
        _ if dt.year() == today.year() => dt.format("%B").to_string().into(),
        _ => dt.format("%B %Y").to_string().into(),
    }
}

impl ListDelegate for SessionsDelegate {
    type Item = ListItem;

    fn sections_count(&self, _cx: &App) -> usize {
        self.groups.len()
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        self.groups.get(section).map_or(0, |(_, r)| r.len())
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let (label, _) = self.groups.get(section)?;
        if label.is_empty() {
            return None;
        }
        let theme = cx.theme();
        Some(
            div()
                .w_full()
                .mx(SPACE_SM)
                // 与详情头「标题 → 元信息」的 gap 同值:首组组头因此与右栏元信息
                // 行齐平。对所有组头必须一致——List 只测量 section 0 的高度再套
                // 给全部组头(list.rs:406);组间分隔靠线,不靠拉大间距
                .pt(SPACE_SM)
                .pb(SPACE_XS)
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(7.))
                        .items_center()
                        .child(
                            div()
                                .w(px(3.))
                                .h(px(11.))
                                .flex_shrink_0()
                                .rounded(px(1.5))
                                .bg(rgb(crate::theme::DATE_GROUP_ACCENT)),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(FONT_LABEL)
                                .font_medium()
                                .text_color(theme.muted_foreground)
                                .child(label.to_uppercase()),
                        )
                        // 发丝线接在文字后面延伸出去,标记"这一组从这里开始"。
                        // 首组上方就是列表顶部,无需与谁分隔,所以不画——
                        // 线高 1px 被文字撑住,去掉它不改变组头高度
                        .when(section > 0, |row| {
                            row.child(div().flex_1().h(px(1.)).bg(theme.border))
                        }),
                ),
        )
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let s = self.sessions.get(self.flat_index(ix)?)?;
        let theme = cx.theme();

        // 行内文字全部按格数自截断(见 clip_display 的注释:gpui 的 truncate
        // 在虚拟列表里不画省略号)。格数按 336px 列宽减去 ListItem 与行内
        // 边距、图标、时间列之后的余量折算,列宽是固定值所以阈值稳定。
        const TITLE_CELLS: usize = 42;
        const PROJECT_CELLS: usize = 14;
        // model 不进列表:336px 的列放不下"项目 + 消息数 + model + 时间",
        // 硬塞会把项目名挤成两三个字。model 在详情页元信息带里
        let facts = if s.message_count > 0 {
            format!("{} messages", s.message_count)
        } else {
            String::new()
        };

        Some(
            // mx 是选中胶囊的左右留白
            ListItem::new(ix.row)
                .rounded(theme.radius)
                .mx(SPACE_SM)
                .child(
                    v_flex()
                        .w_full()
                        .px(SPACE_XS)
                        .py(SPACE_SM)
                        .gap(px(5.))
                        // 标题独占一行,星标/置顶与时间下沉到元信息行
                        .child(
                            div()
                                // nowrap 是兜底:格数是估算值,超了只能裁,绝不
                                // 能换行——List 只测一行高度套给所有行(list.rs:406)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(FONT_BODY)
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(clip_display(&s.title, TITLE_CELLS)),
                        )
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .items_center()
                                .text_size(FONT_LABEL)
                                .text_color(theme.muted_foreground)
                                // agent 图标固定保留,是每行的身份锚点
                                .child(
                                    img(s.agent.brand_icon(theme.mode.is_dark()))
                                        .size(px(14.))
                                        .flex_shrink_0(),
                                )
                                .child(badge(
                                    clip_display(&s.project_name, PROJECT_CELLS),
                                    theme.muted,
                                    theme.muted_foreground,
                                ))
                                .when(!facts.is_empty(), |this| {
                                    this.child(meta_sep(theme.muted_foreground))
                                        .child(div().flex_shrink_0().child(facts.clone()))
                                })
                                .child(div().flex_1())
                                .when(s.pinned, |this| {
                                    this.child(
                                        icon("icons/pin-filled.svg")
                                            .with_size(px(11.))
                                            .flex_shrink_0()
                                            .text_color(theme.primary),
                                    )
                                })
                                .when(s.favorite, |this| {
                                    this.child(
                                        icon("icons/star-filled.svg")
                                            .with_size(px(11.))
                                            .flex_shrink_0()
                                            .text_color(rgb(crate::theme::STAR_YELLOW)),
                                    )
                                })
                                .child(div().flex_shrink_0().child(relative_time(s.updated_at))),
                        ),
                ),
        )
    }

    // 点击/回车走 ListEvent::Confirm(ix),无需自存选中态
    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }
}

// ---------------- 搜索面板 delegate ----------------

pub struct SearchDelegate {
    pub hits: Vec<SearchHit>,
    pub degraded: bool,
    store: Arc<Store>,
    last_query: String,
}

impl ListDelegate for SearchDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.hits.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let h = self.hits.get(ix.row)?;
        let theme = cx.theme();
        let timestamp = h.timestamp.unwrap_or(0);
        let hit_time: SharedString = relative_time(timestamp).into();
        let hit_time_tooltip: SharedString = abs_date(timestamp).into();
        let snippet = h
            .snippet
            .replace(HL_OPEN, "「")
            .replace(HL_CLOSE, "」")
            .replace('\n', " ");
        Some(
            // ListItem 无默认 margin,行块与内容区同宽(胶囊边对齐 Scope 行/
            // 输入行);块内文字缩进保持组件默认(px_3)+ 内容 px_2
            ListItem::new(ix.row).rounded(theme.radius).child(
                v_flex()
                    .w_full()
                    .px(SPACE_SM)
                    .py(SPACE_SM)
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .gap(SPACE_SM)
                            .text_size(FONT_CAPTION)
                            .child(
                                img(h.session.agent.brand_icon(theme.mode.is_dark()))
                                    .size(px(15.))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .font_medium()
                                    .text_color(theme.foreground)
                                    .truncate()
                                    .child(h.session.title.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(("search-hit-time", ix.row))
                                    .flex_shrink_0()
                                    .text_size(FONT_CAPTION)
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} · {}", h.session.project_name, hit_time))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            hit_time_tooltip.clone(),
                                        )
                                        .build(window, cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(FONT_CAPTION)
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(snippet),
                    ),
            ),
        )
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> gpui::Task<()> {
        let q = query.to_string();
        self.last_query = q.clone();
        let store = self.store.clone();
        let bg = cx.background_spawn(async move {
            if q.trim().is_empty() {
                (Vec::new(), false)
            } else {
                store
                    .search(&q, &[], None, 60)
                    .unwrap_or((Vec::new(), false))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let (hits, degraded) = bg.await;
            this.update(cx, |state, cx| {
                let d = state.delegate_mut();
                d.hits = hits;
                d.degraded = degraded;
                cx.notify();
            })
            .ok();
        })
    }

    // 查询为空时的引导页也走这里:搜索框已拆出 List 自管(searchable(false)),
    // ListState 不再有 query_input,render_initial 永不触发
    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        if self.last_query.trim().is_empty() {
            return v_flex()
                .h(px(250.))
                .w_full()
                .justify_center()
                .child(empty_state(
                    "icons/search.svg",
                    px(48.),
                    px(22.),
                    "Search full conversation text",
                    "Matches natural language and code, like \"useEffect(\".",
                    cx,
                ));
        }
        v_flex()
            .h(px(250.))
            .w_full()
            .items_center()
            .justify_center()
            .gap(SPACE_MD)
            .text_color(theme.muted_foreground)
            .child(icon("icons/inbox.svg").with_size(px(24.)))
            .child(
                div()
                    .text_size(FONT_BODY)
                    .font_medium()
                    .child(format!("No results for \"{}\"", self.last_query)),
            )
            .child(
                div()
                    .text_size(FONT_CAPTION)
                    .child("Try a different or shorter query."),
            )
    }

    fn render_section_header(
        &mut self,
        _section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        if !self.degraded {
            return None;
        }
        Some(
            div()
                .px(SPACE_SM)
                .pb(SPACE_XS)
                .text_size(FONT_LABEL)
                .text_color(cx.theme().muted_foreground)
                .child("Short query — using fallback search. Longer keywords are faster."),
        )
    }
}

// ---------------- 详情状态 ----------------

struct DetailState {
    meta: SessionMeta,
    /// 过滤后的可见消息。Rc 让行渲染以引用计数克隆代替整条消息深拷贝
    transcript: Rc<Vec<TranscriptMessage>>,
    loading: bool,
    /// 解析失败的原因。Some 时阅读区渲染错误面板
    error: Option<SharedString>,
    /// 逐消息不等高列表(gpui 原生 ListState,惰性测量)
    msg_list: gpui::ListState,
    /// 展开的工具簇(按消息在 transcript 里的下标)
    expanded_tools: HashSet<usize>,
    /// 展开的 thinking。与工具簇分开存,否则两者会互相带着开
    expanded_thinking: HashSet<usize>,
    /// 搜索跳转目标(FTS seq,契约=消息 seq);解析完成后滚到该消息并保持高亮
    jump_seq: Option<i64>,
    /// 与 transcript 同下标的内联图片。解析时一次性建好 `Arc<Image>` 存住——
    /// gpui 的 `Image` 按字节内容哈希做 id、解码结果有缓存,但每帧重建会把
    /// 整块字节 clone 一遍,几 MB 的截图逐帧 memcpy 扛不住
    images: Vec<Vec<ImageSlot>>,
    /// 放大预览中的图:(消息下标, 该消息内的图片下标)
    zoom: Option<(usize, usize)>,
}

/// 一张图在 UI 侧的两种状态。gpui 只认 `ImageFormat` 那七种格式,
/// HEIC 之类解得出字节也渲染不了,给出说明而不是静默吞掉
#[derive(Clone)]
enum ImageSlot {
    /// `dims` 是原始像素宽高。两个用处:正方形缩略图靠宽高比决定钉宽还是
    /// 钉高(gpui 的 `img()` 没有 object-fit,cover 要自己做),放大预览的
    /// 元信息也要显示它。解不出尺寸时为 None,按横图处理
    Ready {
        image: Arc<gpui::Image>,
        dims: Option<(u32, u32)>,
    },
    Unsupported(SharedString),
}

// ---------------- Workbench ----------------

pub struct Workbench {
    focus_handle: FocusHandle,
    store: Arc<Store>,
    /// 扫描/监听只含启用 location；管理面板另读 data_locations，停用行不消失。
    adapters: SharedAdapters,
    /// 与 adapters 同一次 roster 构造得到的全部 location 路径快照。
    data_locations: SharedLocations,

    selected_agent: Option<AgentId>,
    selected_project: Option<String>,
    favorite_only: bool,
    sort_key: SortKey,
    sort_ascending: bool,

    agent_counts: Vec<(AgentId, i64)>,
    projects: Vec<ProjectInfo>,
    agents_collapsed: bool,
    projects_collapsed: bool,
    starred_count: i64,
    /// 后台扫描的最新状态,启动时的自动扫描与用户主动重扫共用。整份留存而不是
    /// 摊成几个字段:刷新入口的守卫、侧栏状态文案、按钮 busy 态全部从它派生,
    /// 单一写入点就不会互相失步——把派生出的文案反过来当守卫,正是
    /// "扫描失败后刷新按钮再也点不动"的来源
    scan: ScanProgress,
    /// 用户主动发起的重扫。与 scan.scanning 正交：前者决定终态通知，后者是
    /// 所有自动/手动扫描共用的实际运行状态。
    refreshing: bool,
    /// location 变更撞上进行中的扫描时置位:那轮扫描持旧 roster,不补扫的话
    /// 新根不收录、被移根不出清,要等手动 ⌘R(2026-08-24 Codex review)。
    /// 终态事件到达后由 on_bg_event 消费,用新 roster 补一轮增量
    pending_rescan: bool,
    /// Settings 是单例窗口；句柄不保活，关闭后下次点击会检测失败并重建。
    settings_window: Option<AnyWindowHandle>,
    settings_page: SettingsPage,
    update_status: UpdateStatus,
    total_sessions: i64,

    list_state: Entity<ListState<SessionsDelegate>>,
    palette_list: Entity<ListState<SearchDelegate>>,
    /// ⌘K 搜索输入框(自管,不用 List 内置 searchable:清除钮可控)
    palette_input: Entity<InputState>,
    /// 进行中的搜索任务;新输入覆盖旧值即取消过期搜索
    _palette_search_task: Option<Task<()>>,

    detail: Option<DetailState>,

    /// Insights 页(侧栏底部入口):打开时替换中栏+右栏。与其他导航目的地
    /// 互斥(侧栏单选模型);数据在 open/refresh 时后台重算,Rc 免深拷贝
    insights_open: bool,
    insights: Option<Rc<InsightsData>>,
    insights_loading: bool,
    insights_range: InsightsRange,
    /// 三个榜单各自的度量档,按 UsageBoard 序数索引
    insights_metrics: [InsightsMetric; 3],
    /// 进行中的统计查询;新查询覆盖旧值即取消,扫描风暴下不堆积读锁竞争
    insights_task: Option<Task<()>>,

    scan_events: Arc<dyn ScanEvents>,
    watcher: Option<SessionWatcher>,
    /// 终端 id → 提取好的应用图标 png(后台 JXA 提取,详情页 Open In 用)
    terminal_icons: HashMap<String, PathBuf>,
    /// Open In 上次选择(split 按钮左段直开目标),None = 已装列表首个
    preferred_terminal: Option<terminal::TerminalApp>,
    _subs: Vec<Subscription>,
}

/// Insights 分布图的维度(‹ › 循环切换)。数据三份都在 InsightsData 里,
/// 切换是纯视图状态,不触发重查
#[derive(Clone, Copy, PartialEq, Eq)]
enum InsightsRange {
    Hour,
    Weekday,
    Month,
}

impl InsightsRange {
    fn title(self) -> &'static str {
        match self {
            Self::Hour => "By hour",
            Self::Weekday => "By weekday",
            Self::Month => "By month",
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Hour => Self::Month,
            Self::Weekday => Self::Hour,
            Self::Month => Self::Weekday,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Hour => Self::Weekday,
            Self::Weekday => Self::Month,
            Self::Month => Self::Hour,
        }
    }
}

/// 榜单(Agents/Top projects/Models)的度量维度,‹ › 循环切换、每个榜单
/// 各自记忆档位。Tokens 只在组内有人报过用量时进入循环——tokens=0 语义
/// 是"不报"而非"用了 0"
#[derive(Clone, Copy, PartialEq, Eq)]
enum InsightsMetric {
    Sessions,
    Prompts,
    Tokens,
}

impl InsightsMetric {
    fn caption(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Prompts => "Prompts",
            Self::Tokens => "Tokens",
        }
    }

    fn value(self, u: &UsageTally) -> i64 {
        match self {
            Self::Sessions => u.sessions,
            Self::Prompts => u.prompts,
            Self::Tokens => u.tokens,
        }
    }

    fn display(self, u: &UsageTally) -> String {
        match self {
            Self::Tokens => fmt_tokens(Some(u.tokens)),
            _ => thousands(self.value(u)),
        }
    }
}

/// 三个榜单的静态规格。`Workbench::insights_metrics` 按其序数索引各自
/// 记忆档位——同一个渲染方法靠它读写自己的状态,不必外传 setter
#[derive(Clone, Copy)]
enum UsageBoard {
    Agents,
    Projects,
    Models,
}

impl UsageBoard {
    fn title(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Projects => "Projects",
            Self::Models => "Models",
        }
    }

    fn arrow_id(self) -> &'static str {
        match self {
            Self::Agents => "agents-arrow",
            Self::Projects => "projects-arrow",
            Self::Models => "models-arrow",
        }
    }

    /// Agents 全量列出(总共十四家);项目/模型长尾长,取前 6
    fn limit(self) -> usize {
        match self {
            Self::Agents => usize::MAX,
            _ => 6,
        }
    }

    fn name_w(self) -> Pixels {
        match self {
            Self::Models => px(176.),
            _ => px(128.),
        }
    }
}

/// Settings/Locations 的一行(= 一个数据源路径)。文本字段用 SharedString，
/// 让跨窗口快照与菜单闭包的 clone 只做引用计数。
#[derive(Clone)]
pub(crate) struct DataSourceRow {
    pub(crate) agent: AgentId,
    /// `~/…` 展示形态
    pub(crate) display: SharedString,
    /// 原始完整路径,交给 Finder
    pub(crate) raw: SharedString,
    /// 会话数,或路径不可用时的状态词
    pub(crate) tally: SharedString,
    /// 路径当前存在(行是否可点)
    pub(crate) exists: bool,
    /// Some(落库路径) = 自定义行(路径可能是本行根的上层目录);None = 预设行
    pub(crate) custom: Option<SharedString>,
    /// 预设行是否能只压制本路径，而不关闭该 agent 的整个默认实例。
    pub(crate) individual_default: bool,
    pub(crate) enabled: bool,
}

#[derive(Clone)]
pub(crate) struct LocationSettingsSnapshot {
    pub(crate) rows: Vec<DataSourceRow>,
    pub(crate) diverged: bool,
}

#[derive(Clone)]
pub(crate) struct DataSettingsSnapshot {
    pub(crate) display_path: SharedString,
    pub(crate) raw_path: SharedString,
    pub(crate) size_bytes: u64,
    pub(crate) session_count: i64,
}

/// location 表单的语义目标。预设行的“编辑”落库为**压制默认 + 记自定义**；
/// 真正的 Remove 只对自定义 location 出现，预设行由开关启停。
#[derive(Clone)]
enum FormTarget {
    Add,
    /// 编辑既有一行。custom=true:path 是落库的自定义路径(编辑单位);
    /// custom=false:path 是预填表单的路径；root 始终是被点行的真实数据根。
    Edit {
        agent: AgentId,
        path: SharedString,
        root: SharedString,
        custom: bool,
        individual_default: bool,
    },
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let db_path = wake_core::db::default_db_path();
        // 库损坏时降级重建而不是崩掉:GUI 秒退什么都不告诉用户,他们也无从知道
        // 删掉那个文件就能自愈。重建也失败说明是目录权限/磁盘问题,那才没救——
        // 但至少要用系统弹窗把话说清楚再退。
        let (store, db_note) = match wake_core::db::open_or_rebuild(&db_path) {
            Ok(v) => v,
            Err(e) => {
                terminal::show_fatal_alert(&format!(
                    "Wake couldn't open or rebuild its index at {}. {e}",
                    db_path.display()
                ));
                std::process::exit(1);
            }
        };
        let store = Arc::new(store);
        let (adapters, data_locations) = Self::build_roster(&store);

        let list_state = cx.new(|cx| {
            ListState::new(
                SessionsDelegate::new(Vec::new(), SortKey::Updated, false),
                window,
                cx,
            )
            .searchable(false)
        });
        let palette_list = cx.new(|cx| {
            ListState::new(
                SearchDelegate {
                    hits: Vec::new(),
                    degraded: false,
                    store: store.clone(),
                    last_query: String::new(),
                },
                window,
                cx,
            )
            .searchable(false)
        });
        let palette_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search everything \u{2014} prose or code")
        });

        // 后台:全量扫描线程 + 文件监听
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<BgEvent>();
        let events: Arc<dyn ScanEvents> = Arc::new(ChannelEvents(tx));
        spawn_scan(adapters.clone(), store.clone(), events.clone(), false);
        let watcher = start_watcher(adapters.clone(), store.clone(), events.clone());
        let scan_events = events.clone();

        // 事件泵跟 Workbench entity 走，而不是跟主窗口走：Settings 会在主窗口
        // 关闭后继续持有 Workbench；若这里绑定 window，update_in 会失败并让
        // scan.scanning 永远收不到终态，后续 location 变更也无法再补扫。
        let main_window = window.window_handle();
        cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                let note = match this.update(cx, |this, cx| this.on_bg_event(ev, cx)) {
                    Ok(note) => note,
                    Err(_) => break,
                };
                // 主窗口已关闭时状态仍正常收尾，只是不再尝试展示无处承载的
                // 完成通知。后台刷新不应顺手关闭用户正在使用的搜索面板。
                if let Some(note) = note {
                    main_window
                        .update(cx, |_, window, cx| window.push_notification(note, cx))
                        .ok();
                }
            }
        })
        .detach();

        let subs = vec![
            cx.subscribe_in(&list_state, window, Self::on_list_event),
            cx.subscribe_in(&palette_list, window, Self::on_palette_event),
            cx.subscribe_in(&palette_input, window, Self::on_palette_input_event),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            store,
            adapters,
            data_locations,
            selected_agent: None,
            selected_project: None,
            sort_key: SortKey::Updated,
            sort_ascending: false,
            favorite_only: false,
            agent_counts: Vec::new(),
            projects: Vec::new(),
            agents_collapsed: false,
            projects_collapsed: false,
            starred_count: 0,
            // 扫描线程已在上面 spawn,首个 Progress 事件到达前先占位为"扫描中",
            // 否则这个窗口内按 ⌘R 会起第二条并发全量扫描
            scan: ScanProgress {
                scanning: true,
                ..Default::default()
            },
            refreshing: false,
            pending_rescan: false,
            settings_window: None,
            settings_page: SettingsPage::General,
            update_status: UpdateStatus::Idle,
            total_sessions: 0,
            list_state,
            palette_list,
            palette_input,
            _palette_search_task: None,
            detail: None,
            insights_open: false,
            insights: None,
            insights_loading: false,
            insights_range: InsightsRange::Hour,
            insights_metrics: [InsightsMetric::Sessions; 3],
            insights_task: None,
            scan_events,
            watcher,
            terminal_icons: HashMap::new(),
            preferred_terminal: None,
            _subs: subs,
        };
        this.refresh(cx);

        // 索引重建过就告诉用户一声——收藏/置顶没了,总得让人知道为什么。
        // defer 到下一帧:此刻 Root 还没建好,notification 层挂不上
        if let Some(note) = db_note {
            cx.defer_in(window, move |_, window, cx| {
                window.push_notification(Notification::warning(note), cx);
            });
        }

        // 终端应用图标后台提取(首次数百 ms,之后命中缓存)
        let icons_task = cx.background_spawn(async {
            let dir = dirs::data_dir()
                .unwrap_or_default()
                .join("wake")
                .join("app-icons");
            terminal::ensure_app_icons(&dir)
        });
        cx.spawn_in(window, async move |this, cx| {
            let icons = icons_task.await;
            this.update(cx, |this, cx| {
                this.terminal_icons = icons;
                cx.notify();
            })
            .ok();
        })
        .detach();
        this
    }

    // ---------- 数据刷新 ----------

    fn current_filter(&self) -> SessionFilter {
        SessionFilter {
            agents: self.selected_agent.into_iter().collect(),
            project_path: self.selected_project.clone(),
            favorite_only: self.favorite_only,
            include_archived: false,
            title_query: None,
            sort: self.sort_key,
            ascending: self.sort_ascending,
            // 列表是虚拟滚动的,这个上限只为兜住内存;超出时中栏底部会明说
            limit: 2000,
            offset: 0,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let filter = self.current_filter();
        if let Ok((sessions, total)) = self.store.list_sessions(&filter) {
            self.total_sessions = total;
            let (sort, ascending) = (self.sort_key, self.sort_ascending);
            self.list_state.update(cx, |state, cx| {
                // 整体换 delegate:分组由排序方式与时间戳推出,只换数据会
                // 留下上一轮的分组区间
                *state.delegate_mut() = SessionsDelegate::new(sessions, sort, ascending);
                cx.notify();
            });
        }
        let mut counts: Vec<(AgentId, i64)> = self
            .store
            .agent_counts()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| AgentId::from_str(&k).map(|a| (a, v)))
            .collect();
        // 固定排序(AgentId 声明序):按会话数排会在平局时抖动
        // (HashMap 迭代无序),每次刷新侧栏顺序都会跳
        counts.sort_by_key(|&(a, _)| a);
        self.agent_counts = counts;
        self.projects = self.store.list_projects().unwrap_or_default();
        self.starred_count = self.store.starred_count().unwrap_or(0);
        // Insights 打开着就顺带重算:扫描增量/收藏变更等一切走 refresh 的
        // 路径都会让页面数据跟上,不设第二条失效通道
        self.reload_insights(cx);
        cx.notify();
    }

    /// 侧栏底部入口。再点一次(或点任意导航行)退回会话列表
    fn toggle_insights(&mut self, cx: &mut Context<Self>) {
        if self.insights_open {
            self.insights_open = false;
            cx.notify();
            return;
        }
        self.insights_open = true;
        // 互斥单选:Insights 是独立目的地,退出时落回 All Sessions
        self.selected_agent = None;
        self.selected_project = None;
        self.favorite_only = false;
        self.refresh(cx);
    }

    /// messages 全表分桶几十毫秒量级,走后台;已有数据时静默换新不闪 loading。
    /// 扫描进行中 Changed 事件每秒都来,有旧数据就先按住——终态 Progress
    /// 会补最后一次;新任务覆盖 insights_task 即取消旧查询,不堆积读锁竞争
    fn reload_insights(&mut self, cx: &mut Context<Self>) {
        if !self.insights_open {
            return;
        }
        if self.scan.scanning && self.insights.is_some() {
            return;
        }
        self.insights_loading = self.insights.is_none();
        let store = self.store.clone();
        let task =
            cx.background_spawn(async move { store.insights(chrono::Local::now().date_naive()) });
        self.insights_task = Some(cx.spawn(async move |this, cx| {
            let data = task.await;
            this.update(cx, |this, cx| {
                if let Ok(data) = data {
                    this.insights = Some(Rc::new(data));
                }
                this.insights_loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// 手动全量重扫(菜单 File → Refresh Sessions,⌘R)。刷新中忽略重复触发。
    /// 扫描在后台进行，侧栏持续显示进度；浏览、搜索和阅读不被阻断。
    fn refresh_sessions(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.scan.scanning {
            return;
        }
        self.scan = ScanProgress {
            scanning: true,
            ..Default::default()
        };
        self.refreshing = true;
        cx.notify();
        spawn_scan(
            self.adapters.clone(),
            self.store.clone(),
            self.scan_events.clone(),
            true,
        );
    }

    /// Settings/Locations 页的数据快照。路径与 active roster 来自同一次
    /// adapter 构造，因此停用项仍然可见，环境变量根也不会二次探测后错位。
    pub(crate) fn location_settings_snapshot(&self) -> LocationSettingsSnapshot {
        let mut flat = self.data_locations.as_ref().clone();
        // 自定义根紧随所属 agent 的默认根；同家内部保持 adapter 声明顺序。
        flat.sort_by_key(|location| location.agent);
        let prefixes: Vec<(String, String)> = flat
            .iter()
            .map(|location| {
                (
                    location.agent.as_str().to_string(),
                    location.path.to_string_lossy().to_string(),
                )
            })
            .collect();
        let counts = self
            .store
            .counts_by_path_prefix(&prefixes)
            .unwrap_or_else(|_| vec![0; prefixes.len()]);
        let (customs, removed, removed_roots) = self.store.location_overrides();
        let customs: Vec<(AgentId, SharedString)> = customs
            .into_iter()
            .map(|(agent, path)| {
                (
                    agent,
                    SharedString::from(path.to_string_lossy().to_string()),
                )
            })
            .collect();
        let rows = flat
            .iter()
            .zip(prefixes)
            .zip(counts)
            .map(|((location, (_, raw)), count)| {
                let exists = location.path.exists();
                DataSourceRow {
                    agent: location.agent,
                    display: tilde_path(&raw).into(),
                    raw: raw.clone().into(),
                    tally: if exists {
                        match count {
                            1 => "1 session".into(),
                            n => format!("{n} sessions").into(),
                        }
                    } else {
                        "Folder not found".into()
                    },
                    exists,
                    custom: custom_owner(&customs, location.agent, &raw).cloned(),
                    individual_default: location.individually_removable,
                    enabled: location.enabled,
                }
            })
            .collect();
        let diverged = !customs.is_empty()
            || !removed.is_empty()
            || !removed_roots.is_empty()
            || self.data_locations.iter().any(|location| !location.enabled);
        LocationSettingsSnapshot { rows, diverged }
    }

    pub(crate) fn data_settings_snapshot(&self) -> DataSettingsSnapshot {
        let path = wake_core::db::default_db_path();
        let raw = path.to_string_lossy().to_string();
        let size_bytes = ["", "-wal", "-shm"]
            .iter()
            .filter_map(|suffix| std::fs::metadata(format!("{raw}{suffix}")).ok())
            .map(|metadata| metadata.len())
            .sum();
        DataSettingsSnapshot {
            display_path: tilde_path(&raw).into(),
            raw_path: raw.into(),
            size_bytes,
            session_count: self.agent_counts.iter().map(|(_, count)| count).sum(),
        }
    }

    pub(crate) fn settings_page(&self) -> SettingsPage {
        self.settings_page
    }

    pub(crate) fn update_status(&self) -> &UpdateStatus {
        &self.update_status
    }

    pub(crate) fn select_settings_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_page = page;
        cx.notify();
    }

    pub(crate) fn open_about(&mut self, cx: &mut Context<Self>) {
        self.settings_page = SettingsPage::About;
        cx.notify();
        self.open_settings(cx);
    }

    pub(crate) fn open_updates(&mut self, cx: &mut Context<Self>) {
        self.settings_page = SettingsPage::Updates;
        cx.notify();
        self.open_settings(cx);
        self.check_for_updates(cx);
    }

    pub(crate) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_status, UpdateStatus::Checking) {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        cx.notify();

        // reqwest::blocking 会自行创建 Tokio runtime；若直接放进 GPUI 的异步
        // executor，runtime 嵌套会 panic，界面便会永远停在 Checking。让阻塞
        // 请求待在独立系统线程里，再用 oneshot 把结果送回 UI executor。
        let (sender, receiver) = futures::channel::oneshot::channel();
        std::thread::spawn(|| {
            let _ = sender.send(update::check_latest_release(env!("CARGO_PKG_VERSION")));
        });
        cx.spawn(async move |this, cx| {
            let status = match receiver.await {
                Ok(Ok(info)) if info.update_available => UpdateStatus::Available {
                    latest: info.latest_version.to_string(),
                },
                Ok(Ok(info)) => UpdateStatus::UpToDate {
                    latest: info.latest_version.to_string(),
                },
                Ok(Err(error)) => {
                    eprintln!("update check failed: {error:#}");
                    UpdateStatus::Failed
                }
                Err(_) => UpdateStatus::Failed,
            };
            this.update(cx, |this, cx| {
                this.update_status = status;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 打开单例 Settings 窗口。开窗必须 defer 到当前 Workbench update 退出
    /// 之后：Settings 首帧会读取 location 快照，同步开窗会反读仍被独占借用
    /// 的 Workbench，触发 GPUI double lease。
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        let workbench = cx.entity();
        cx.defer(move |cx| Self::show_settings_window(workbench, cx));
    }

    fn show_settings_window(workbench: Entity<Self>, cx: &mut App) {
        // 先把 Copy 句柄取出，确保 read lease 在后续 update 前结束。
        let existing = workbench.read(cx).settings_window;
        if let Some(handle) = existing {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                cx.activate(true);
                return;
            }
            workbench.update(cx, |this, _| this.settings_window = None);
        }
        let bounds = Bounds::centered(None, size(px(820.), px(600.)), cx);
        let titlebar = if cfg!(target_os = "macos") {
            TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(20.), px(11.))),
            }
        } else {
            TitlebarOptions {
                title: Some("Wake Settings".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }
        };
        let settings_workbench = workbench.clone();
        match cx.open_window(
            WindowOptions {
                titlebar: Some(titlebar),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(520.))),
                app_id: Some("wake-settings".into()),
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            move |window, cx| {
                window
                    .observe_window_appearance(|window, cx| {
                        crate::theme::sync_appearance(Some(window), cx);
                    })
                    .detach();
                crate::theme::sync_appearance(Some(window), cx);
                let settings = cx.new(|cx| SettingsView::new(settings_workbench, window, cx));
                window.focus(&settings.read(cx).focus_handle(cx));
                cx.new(|cx| Root::new(settings, window, cx))
            },
        ) {
            Ok(handle) => {
                workbench.update(cx, |this, _| this.settings_window = Some(handle.into()));
                // 逐窗口属性,新窗口要再关一次
                crate::macos::suppress_titlebar_separator();
                cx.activate(true);
            }
            Err(error) => eprintln!("failed to open Wake settings: {error}"),
        }
    }

    /// store 的 location 配置 → active roster + 全量路径快照。new 与
    /// rebuild_roster 共用；解析与组装都在 wake-core，与 scan CLI 同一条路。
    fn build_roster(store: &Arc<Store>) -> (SharedAdapters, SharedLocations) {
        let roster = create_adapter_roster_for(store);
        (Arc::new(roster.active), Arc::new(roster.locations))
    }

    /// location 配置变更后的唯一 roster 换代点:同一处换 Arc + 重启 watcher,
    /// 新旧两份实例不共存(不变量 8 的运行时补充:"单实例"指任一时刻只有一份
    /// 在服务,换代必须整体换、所有消费方跟随新 Arc)
    fn rebuild_roster(&mut self, cx: &mut Context<Self>) {
        // 先撤旧 watcher 并等它退出(SessionWatcher::Drop 内 join):旧线程持
        // 旧 roster,不等收尾,已移除根的会话可能在补扫后被写回复活
        self.watcher = None;
        (self.adapters, self.data_locations) = Self::build_roster(&self.store);
        self.watcher = start_watcher(
            self.adapters.clone(),
            self.store.clone(),
            self.scan_events.clone(),
        );
        cx.notify();
    }

    /// location 表单(添加/编辑共用一套 UI,2026-08-24 定稿):agent 下拉 +
    /// 路径输入框(可手输,~ 展开)+ 目录选择按钮;Cancel/Save 只在有改动时
    /// 出现。表单作为 Settings 窗口的模态层，esc/取消回到 Locations 页。
    pub(crate) fn open_add_location_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_location_form(FormTarget::Add, window, cx);
    }

    pub(crate) fn open_edit_location_form(
        &mut self,
        row: DataSourceRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // location 的配置单位是目录；SQLite 型 adapter 的展示根可能是文件，
        // 编辑时预填父目录，避免保存成 <db>/<db>。
        let path = row.custom.clone().unwrap_or_else(|| {
            let raw = row.raw.as_ref();
            if std::path::Path::new(raw).is_file() {
                std::path::Path::new(raw)
                    .parent()
                    .map(|path| SharedString::from(path.to_string_lossy().to_string()))
                    .unwrap_or_else(|| row.raw.clone())
            } else {
                row.raw.clone()
            }
        });
        self.open_location_form(
            FormTarget::Edit {
                agent: row.agent,
                path,
                root: row.raw,
                custom: row.custom.is_some(),
                individual_default: row.individual_default,
            },
            window,
            cx,
        );
    }

    fn open_location_form(
        &mut self,
        target: FormTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, ok_label, init_agent, init_path): (
            &'static str,
            &'static str,
            AgentId,
            SharedString,
        ) = match &target {
            FormTarget::Add => ("Add location", "Add", AgentId::ClaudeCode, "".into()),
            FormTarget::Edit { agent, path, .. } => ("Edit location", "Save", *agent, path.clone()),
        };
        // 占位符须与校验规则(Path::is_absolute)同形:Windows 上没有盘符
        // 的 `/absolute/...` 并不算绝对路径,照着占位符敲会被拒
        let placeholder = if cfg!(target_os = "windows") {
            r"C:\absolute\folder\path"
        } else {
            "/absolute/folder/path"
        };
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        if !init_path.is_empty() {
            let v = init_path.clone();
            path_input.update(cx, |st, cx| st.set_value(v, window, cx));
        }
        // 编辑态动作行的 Finder 只挂真实存在的路径——目录或 SQLite 库文件都算
        // (open_in_finder 对文件走 reveal 选中;用 is_dir 会把三家库文件行的
        // Finder 恒隐藏,2026-08-24 Codex review)。fs 探测一次,不进每帧 builder
        let edit_exists = match &target {
            FormTarget::Add => None,
            FormTarget::Edit { path, .. } => Some(std::path::Path::new(path.as_ref()).exists()),
        };
        // 表单状态放 Rc<Cell>/entity 而非宿主字段:builder 每帧重跑,闭包内
        // read 宿主 entity 必 double-lease panic(与 refresh 进度弹窗同一约束)
        let selected: Rc<Cell<AgentId>> = Rc::new(Cell::new(init_agent));
        let dirty_init_agent = init_agent;
        let dirty_init_path = init_path;
        let entity = cx.entity();
        let title: SharedString = title.into();
        let ok_label: SharedString = ok_label.into();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let theme = cx.theme();
            let dark = theme.mode.is_dark();
            // 内容轴缩进 = small 按钮的水平内边距:字段行/标题/footer 以它为轴,
            // 动作行的胶囊钮**不缩进**——可见内容(内边距之后)恰好落轴,hover
            // 胶囊完整留在内容盒内。负 margin 会溢出被裁(locations 面板同一教训)
            let field_inset = BUTTON_SM_PX;
            let sel = selected.get();
            // 脏状态:与初始 (agent, path) 有差且路径非空才亮出 Cancel/Save
            // ——没改动时无可保存,收手走 esc/关闭即可(2026-08-24 用户定稿)。
            // Rope 直接与 &str 比较 + chars 扫描:builder 每帧跑,禁 to_string
            let dirty = {
                let text = path_input.read(cx).text();
                (sel != dirty_init_agent || *text != dirty_init_path.as_ref())
                    && text.chars().any(|c| !c.is_whitespace())
            };
            let sel_cell = selected.clone();
            let browse_entity = entity.clone();
            let browse_input = path_input.clone();
            let ok_entity = entity.clone();
            let ok_input = path_input.clone();
            let ok_sel = selected.clone();
            let ok_target = target.clone();
            let action_target = target.clone();
            let dialog = dialog
                .title(
                    div()
                        .pl(field_inset)
                        .text_size(FONT_HEADING)
                        .font_semibold()
                        .child(title.clone()),
                )
                .w(px(500.))
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default().ok_text(ok_label.clone()),
                )
                .child(
                    v_flex()
                        .gap(SPACE_MD)
                        .child(
                            h_flex()
                                .px(field_inset)
                                .gap(SPACE_SM)
                                .items_center()
                                .child(
                                    div()
                                        .w(FORM_LABEL_W)
                                        .flex_shrink_0()
                                        .text_size(FONT_CAPTION)
                                        .text_color(theme.muted_foreground)
                                        .child("Agent"),
                                )
                                .child(
                                    // Button 走 ParentElement 自组内容:品牌图标是
                                    // PNG(img),进不了 .icon()(那只收单色 SVG),
                                    // 图标+名字+箭头必须同住按钮内(2026-08-24 反馈)
                                    Button::new("loc-agent")
                                        .outline()
                                        .rounded(RADIUS_BUTTON)
                                        .child(
                                            h_flex()
                                                .gap(SPACE_SM)
                                                .items_center()
                                                .child(
                                                    img(sel.brand_icon(dark))
                                                        .size(px(14.))
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(FONT_CAPTION)
                                                        .child(sel.display_name()),
                                                )
                                                .child(
                                                    icon("icons/chevron-down.svg")
                                                        .with_size(px(12.))
                                                        .text_color(theme.muted_foreground),
                                                ),
                                        )
                                        .dropdown_menu(move |menu, _, _| {
                                            let mut menu = menu.min_w(px(200.));
                                            for a in AgentId::ALL {
                                                let cell = sel_cell.clone();
                                                // element 变体:菜单项带品牌 PNG
                                                //(纯文本项的 icon 只收单色 SVG)
                                                menu = menu.item(
                                                    PopupMenuItem::element(move |_, _| {
                                                        h_flex()
                                                            .gap(SPACE_SM)
                                                            .items_center()
                                                            .child(
                                                                img(a.brand_icon(dark))
                                                                    .size(px(14.))
                                                                    .flex_shrink_0(),
                                                            )
                                                            .child(a.display_name())
                                                    })
                                                    .checked(sel == a)
                                                    .on_click(move |_, window, _| {
                                                        cell.set(a);
                                                        window.refresh();
                                                    }),
                                                );
                                            }
                                            menu
                                        }),
                                ),
                        )
                        .child(
                            h_flex()
                                .px(field_inset)
                                .gap(SPACE_SM)
                                .items_center()
                                .child(
                                    div()
                                        .w(FORM_LABEL_W)
                                        .flex_shrink_0()
                                        .text_size(FONT_CAPTION)
                                        .text_color(theme.muted_foreground)
                                        .child("Folder"),
                                )
                                .child(div().flex_1().min_w_0().child(Input::new(&browse_input)))
                                .child(
                                    Button::new("loc-browse")
                                        .outline()
                                        .rounded(RADIUS_BUTTON)
                                        .icon(icon("icons/folder.svg").with_size(px(13.)))
                                        .tooltip("Choose a folder")
                                        .on_click({
                                            let entity = browse_entity.clone();
                                            let input = browse_input.clone();
                                            move |_, window, cx| {
                                                let input = input.clone();
                                                entity.update(cx, |this, cx| {
                                                    this.browse_for_location(input, window, cx)
                                                });
                                            }
                                        }),
                                ),
                        )
                        .when_some(edit_exists, |el, exists| {
                            let FormTarget::Edit {
                                agent,
                                path,
                                root: _,
                                custom,
                                individual_default: _,
                            } = action_target.clone()
                            else {
                                unreachable!("edit_exists 仅在 Edit 目标下为 Some")
                            };
                            let remove_entity = entity.clone();
                            el.when(custom || exists, |el| {
                                el.child(
                                    // 动作行遵循破坏性靠左惯例:Remove 靠左、Show in
                                    // Finder 靠右。Remove 只属于真正可删除的自定义
                                    // location；内置 location 由行内开关停用，不删除。
                                    // 两钮手排:内边距 = 轴缩进,Remove
                                    // 左侧再减 1.5 补 lucide 字形内白(24 视框留 3
                                    // 单位),字形左缘正落标签轴;右钮文字右缘正落
                                    // 浏览钮右缘。全正值内边距,胶囊完整在内容盒里
                                    h_flex()
                                        .pt(SPACE_XS)
                                        .items_center()
                                        .justify_between()
                                        .when(custom, |el| {
                                            el.child(
                                                h_flex()
                                                    .id("loc-remove")
                                                    .h(BUTTON_SM_H)
                                                    .pl(BUTTON_SM_PX - px(1.5))
                                                    .pr(BUTTON_SM_PX)
                                                    .rounded(RADIUS_BUTTON)
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .cursor_pointer()
                                                    .text_size(FONT_BODY)
                                                    .text_color(theme.danger)
                                                    .hover(|s| s.bg(theme.danger.opacity(0.1)))
                                                    .active(|s| s.bg(theme.danger.opacity(0.16)))
                                                    .on_click({
                                                        let remove_entity = remove_entity.clone();
                                                        let stored = path.clone();
                                                        move |_, window, cx| {
                                                            // 整栈收场(表单+过期面板);
                                                            // delete 内会重开新快照面板
                                                            window.close_all_dialogs(cx);
                                                            let stored = stored.clone();
                                                            remove_entity.update(cx, |this, cx| {
                                                                this.delete_location(
                                                                    agent, stored, window, cx,
                                                                )
                                                            });
                                                        }
                                                    })
                                                    .child(
                                                        icon("icons/trash-2.svg")
                                                            .with_size(px(13.))
                                                            .flex_shrink_0(),
                                                    )
                                                    .child("Remove"),
                                            )
                                        })
                                        .when(exists, |el| {
                                            el.child(
                                                h_flex()
                                                    .id("loc-reveal")
                                                    .h(BUTTON_SM_H)
                                                    .px(BUTTON_SM_PX)
                                                    .rounded(RADIUS_BUTTON)
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .cursor_pointer()
                                                    .text_size(FONT_BODY)
                                                    .text_color(theme.foreground)
                                                    .hover(|s| s.bg(theme.secondary_hover))
                                                    .active(|s| s.bg(theme.secondary_active))
                                                    .on_click(move |_, _, _| {
                                                        terminal::open_in_file_manager(&path)
                                                    })
                                                    .child(
                                                        icon("icons/folder.svg")
                                                            .with_size(px(13.))
                                                            .flex_shrink_0()
                                                            .text_color(theme.muted_foreground),
                                                    )
                                                    .child(SHOW_IN_FM),
                                            )
                                        }),
                                )
                            })
                        }),
                )
                .on_ok(move |_, window, cx| {
                    let path_text = ok_input.read(cx).text().to_string();
                    let agent = ok_sel.get();
                    ok_entity.update(cx, |this, cx| {
                        this.commit_location_form(ok_target.clone(), agent, path_text, window, cx)
                    })
                });
            if dirty {
                dialog.footer(move |ok, cancel, window, cx| {
                    vec![h_flex()
                        .w_full()
                        .justify_end()
                        .gap(SPACE_SM)
                        .px(field_inset)
                        .child(cancel(window, cx))
                        .child(ok(window, cx))]
                })
            } else {
                dialog
            }
        });
    }

    /// 表单的目录选择按钮:系统选择器,选中即回填输入框(取消无事发生)
    fn browse_for_location(
        &mut self,
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let dir = match rx.await {
                Ok(Ok(Some(paths))) if !paths.is_empty() => paths.into_iter().next().unwrap(),
                _ => return,
            };
            let text = dir.to_string_lossy().to_string();
            this.update_in(cx, |_, window, cx| {
                input.update(cx, |st, cx| st.set_value(text, window, cx));
            })
            .ok();
        })
        .detach();
    }

    /// 表单落库。返回值交给 on_ok:false = 表单留着(校验没过,或已手工收场)。
    /// 纯路径管理:不校验目录内容(2026-08-24 用户定稿),只拒空/相对路径与
    /// 同家重叠;预设行的编辑落库为"压默认 + 记自定义"
    fn commit_location_form(
        &mut self,
        target: FormTarget,
        agent_new: AgentId,
        raw_text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let expanded = expand_tilde(raw_text.trim());
        // Windows 上把手输的 '/' 折成 '\':`~/.claude` 展开后是
        // `C:\Users\me/.claude` 这种混分隔符形态,而 path_owns 的重叠判定是
        // 字节精确比较、explorer 只认反斜杠——不在入口归一,同一目录就会以
        // 两种拼写各注册一份(POSIX 不动:'\' 在那边是合法文件名字符)
        let expanded = if cfg!(target_os = "windows") {
            expanded.replace('/', "\\")
        } else {
            expanded
        };
        // 绝对性先判、只判一次:is_absolute 三端同判据(starts_with('/') 会把
        // 所有 Windows 盘符路径误拒),空串它也判 false,无需另设 is_empty 关。
        // **必须判在剪尾之前**:`//` 剪完是空串,若拿剪后的结果去判就会退回
        // 未剪形态放行,而旧版是拒的(2026-08-25 review)
        if !std::path::Path::new(&expanded).is_absolute() {
            window.push_notification(Notification::warning("Enter an absolute folder path"), cx);
            return false;
        }
        // 尾分隔符归一(展示与重叠判定都吃这份);裸根("/"、"C:\")剪完会
        // 失去绝对性,原样保留
        let trimmed = expanded.trim_end_matches(std::path::is_separator);
        let path = if std::path::Path::new(trimmed).is_absolute() {
            trimmed.to_string()
        } else {
            expanded.clone()
        };
        // 各家归一化(codex:直选 sessions 树/平铺 archived 上提到家层,侧档
        // 找回)。静态分派,不依赖该家实例是否还在 roster(默认被移除时也要
        // 生效);归一化后再做无改动/重叠判定,选中默认根数据子目录会正确判"已覆盖"
        let path =
            wake_core::adapters::normalize_custom_root(agent_new, std::path::PathBuf::from(&path))
                .to_string_lossy()
                .to_string();
        // 没改就没事:直接让机制关表单,面板未过期。旧目标也要**同规归一化**
        // 再比——默认 Codex 的 sessions/archived 行原路径归一化后即 home,不归一
        // 化就比,单按 Enter 会被误判成编辑、静默把默认改成"压默认+记自定义"
        //(2026-08-24 Codex review)
        let unchanged = match &target {
            FormTarget::Add => false,
            FormTarget::Edit { agent, path: p, .. } => {
                *agent == agent_new
                    && wake_core::adapters::normalize_custom_root(
                        *agent,
                        std::path::PathBuf::from(p.as_ref()),
                    )
                    .to_string_lossy()
                        == path
            }
        };
        if unchanged {
            return true;
        }
        // 同家重叠检查,排除被编辑单元自身派生的根(自定义单元 = 其落库路径
        // 之下的根;预设单元 = 不属于任何该家自定义的根)。嵌进**别家**树里
        // 是合法场景(env 根的先例),同家嵌套才是重复读取
        let customs: Vec<(AgentId, SharedString)> = self
            .store
            .location_overrides()
            .0
            .into_iter()
            .map(|(a, p)| (a, SharedString::from(p.to_string_lossy().to_string())))
            .collect();
        let covered = self
            .data_locations
            .iter()
            .filter(|location| location.agent == agent_new)
            .any(|location| {
                let rs = location.path.to_string_lossy().to_string();
                let excluded = match &target {
                    FormTarget::Add => false,
                    FormTarget::Edit {
                        agent,
                        path: unit,
                        custom: true,
                        ..
                    } => *agent == agent_new && path_owns(unit.as_ref(), &rs),
                    FormTarget::Edit {
                        agent,
                        root,
                        custom: false,
                        individual_default: true,
                        ..
                    } => *agent == agent_new && rs == root.as_ref(),
                    FormTarget::Edit {
                        agent,
                        custom: false,
                        ..
                    } => *agent == agent_new && custom_owner(&customs, agent_new, &rs).is_none(),
                };
                !excluded && (path_owns(&path, &rs) || path_owns(&rs, &path))
            });
        if covered {
            window.push_notification(
                Notification::info("This folder is already in Wake's session locations"),
                cx,
            );
            return false;
        }
        let res = match &target {
            FormTarget::Add => self.store.add_custom_root(agent_new.as_str(), &path),
            // 全形态单事务(含换 agent 的编辑):半程失败不得把配置改成半生效
            //(Codex review P2)
            FormTarget::Edit {
                agent,
                path: old,
                root,
                custom,
                individual_default,
            } => self.store.replace_location(
                agent.as_str(),
                custom.then(|| old.as_ref()),
                (!custom && *individual_default).then(|| root.as_ref()),
                root.as_ref(),
                agent_new.as_str(),
                &path,
            ),
        };
        if let Err(e) = res {
            window.push_notification(Notification::error(format!("Save failed: {e}")), cx);
            return false;
        }
        // Settings 页本身是动态快照，只需收起表单；Workbench notify 会让
        // SettingsView 观察者刷新列表。
        window.close_all_dialogs(cx);
        let note = match &target {
            FormTarget::Add => "Location added",
            FormTarget::Edit { .. } => "Location updated",
        };
        self.apply_location_change(Ok(()), "", Notification::success(note), window, cx);
        false
    }

    /// 真正删除一个自定义 location。内置 location 不提供 Remove，只能用
    /// 行内开关暂时停用；磁盘文件始终不动。
    pub(crate) fn delete_location(
        &mut self,
        agent: AgentId,
        stored: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let res = self
            .store
            .remove_custom_root(agent.as_str(), stored.as_ref());
        self.apply_location_change(
            res,
            "Remove failed",
            Notification::info("Location removed"),
            window,
            cx,
        );
    }

    /// Restore defaults:清空全部偏离（自定义、被移除的预设与停用状态），
    /// 回到全部启用的内置默认 location。
    pub(crate) fn restore_default_locations(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let res = self.store.clear_location_overrides();
        self.apply_location_change(
            res,
            "Restore failed",
            Notification::info("Locations restored to defaults"),
            window,
            cx,
        );
    }

    /// Session locations 行内开关。状态先落库，再整体换 active roster 与 watcher。
    pub(crate) fn set_location_enabled(
        &mut self,
        agent: AgentId,
        path: SharedString,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self
            .store
            .set_location_enabled(agent.as_str(), path.as_ref(), enabled)
        {
            Err(e) => window.push_notification(
                Notification::error(format!("Couldn't update location: {e}")),
                cx,
            ),
            Ok(()) => {
                self.rebuild_roster(cx);
                self.kick_incremental_scan(cx);
                window.push_notification(
                    Notification::info(if enabled {
                        "Location enabled"
                    } else {
                        "Location disabled"
                    }),
                    cx,
                );
                window.refresh();
            }
        }
    }

    /// location 变更的统一收尾(删/恢复/表单提交成功共用)。Settings 页
    /// 观察 Workbench 的 notify 并重读快照，不需要关闭/重开管理面板。
    fn apply_location_change(
        &mut self,
        res: anyhow::Result<()>,
        err_prefix: &'static str,
        ok_note: Notification,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match res {
            Err(e) => {
                window.push_notification(Notification::error(format!("{err_prefix}: {e}")), cx);
            }
            Ok(()) => {
                self.rebuild_roster(cx);
                self.kick_incremental_scan(cx);
                window.push_notification(ok_note, cx);
            }
        }
    }

    /// roster 换代后补一轮增量:新根收录、被移根出清。撞上进行中的扫描
    /// 撞上进行中的扫描则排队，由终态事件补扫。
    fn kick_incremental_scan(&mut self, cx: &mut Context<Self>) {
        if self.scan.scanning {
            self.pending_rescan = true;
            return;
        }
        self.scan = ScanProgress {
            scanning: true,
            ..Default::default()
        };
        cx.notify();
        spawn_scan(
            self.adapters.clone(),
            self.store.clone(),
            self.scan_events.clone(),
            false,
        );
    }

    fn on_bg_event(&mut self, ev: BgEvent, cx: &mut Context<Self>) -> Option<Notification> {
        match ev {
            BgEvent::Progress(p) => {
                let note = if !p.scanning && self.refreshing {
                    self.refreshing = false;
                    Some(match &p.error {
                        None => Notification::success("Sessions refreshed"),
                        Some(err) => Notification::error(format!("Refresh failed: {err}")),
                    })
                } else {
                    None
                };
                self.scan = p;
                // 扫描期间发生过 location 变更:那轮用的是旧 roster,
                // 终态一到立刻用当前 roster 补一轮增量
                if !self.scan.scanning && self.pending_rescan {
                    self.pending_rescan = false;
                    self.kick_incremental_scan(cx);
                }
                // 扫描期间 reload_insights 被按住(见其注释),终态补最后一次
                if !self.scan.scanning {
                    self.reload_insights(cx);
                }
                cx.notify();
                note
            }
            BgEvent::Changed => {
                self.refresh(cx);
                None
            }
        }
    }

    // ---------- 事件处理 ----------

    fn on_list_event(
        &mut self,
        list: &Entity<ListState<SessionsDelegate>>,
        ev: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = match ev {
            ListEvent::Select(ix) | ListEvent::Confirm(ix) => *ix,
            ListEvent::Cancel => return,
        };
        let key = {
            let delegate = list.read(cx).delegate();
            delegate
                .flat_index(ix)
                .and_then(|flat| delegate.sessions.get(flat))
                .map(|s| s.key.clone())
        };
        if let Some(key) = key {
            self.open_detail(&key, None, window, cx);
        }
    }

    fn on_palette_event(
        &mut self,
        _list: &Entity<ListState<SearchDelegate>>,
        ev: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ListEvent::Confirm(ix) => self.open_palette_hit(ix.row, window, cx),
            // Select 仅表示高亮移动,不打开
            ListEvent::Select(_) => {}
            // 焦点在结果列表内(鼠标点过行)时 esc 走这里;焦点在输入框时
            // esc 由 Input 冒泡给 Dialog 的 keyboard Cancel 关闭
            ListEvent::Cancel => window.close_dialog(cx),
        }
    }

    pub fn toggle_search(&mut self, _: &ToggleSearch, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let list = self.palette_list.clone();
        let input = self.palette_input.clone();
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let theme = cx.theme();
            let has_query = input.read(cx).text().len() > 0;
            // 输入框尺寸;清除钮的 suffix 补偿 margin 从它派生,改档自动跟随
            let input_size = gpui_component::Size::Large;
            dialog
                .w(px(680.))
                .margin_top(px(72.))
                // Dialog 默认内容 padding 24px 四边;水平 20,用户定稿(2026-08-18)
                .px(SPACE_XL)
                .close_button(false)
                .overlay_closable(true)
                .child(
                    v_flex()
                        // ↑↓ 在 Input 内不被消费,冒泡到这里走 main.rs 的
                        // PALETTE_CONTEXT 键位(Input 拆出 List 后原生 List 绑定够不着)
                        .key_context(PALETTE_CONTEXT)
                        .on_action(window.listener_for(
                            &this,
                            |wb: &mut Self, _: &PaletteUp, window, cx| {
                                wb.palette_move(-1, window, cx)
                            },
                        ))
                        .on_action(window.listener_for(
                            &this,
                            |wb: &mut Self, _: &PaletteDown, window, cx| {
                                wb.palette_move(1, window, cx)
                            },
                        ))
                        // 定高 + 列表 flex_1:输入行/footer 尺寸变化时列表自适应,
                        // 不用手工重算列表高度
                        .h(PALETTE_HEIGHT)
                        .gap(SPACE_MD)
                        .child(
                            div()
                                .flex_shrink_0()
                                .px(SPACE_SM)
                                .border_b_1()
                                .border_color(theme.border)
                                .child(
                                    Input::new(&input)
                                        .with_size(input_size)
                                        .prefix(
                                            icon("icons/search.svg")
                                                .with_size(px(16.))
                                                .text_color(theme.muted_foreground),
                                        )
                                        // 清除钮自绘,不用内置 cleanable:内置钮固定
                                        // xsmall(实渲 10.5px 图标)且无尺寸配置口
                                        .when(has_query, |i| {
                                            i.suffix(
                                                div()
                                                    .id("palette-clear")
                                                    .size(px(24.))
                                                    // 抵消组件对 suffix 区强加的
                                                    // pr(input_px(size)),它在 p_0
                                                    // 之后应用盖不掉;不抵消则清除钮
                                                    // 右缩进与左侧放大镜不对称
                                                    .mr(-input_size.input_px())
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(theme.radius)
                                                    .cursor_pointer()
                                                    .text_color(theme.muted_foreground)
                                                    // 图标-only:尺寸走 with_size,
                                                    // hover 裸改色不踩 text 替换陷阱
                                                    .hover(|s| {
                                                        s.bg(theme.secondary_hover)
                                                            .text_color(theme.foreground)
                                                    })
                                                    .on_click({
                                                        let input = input.clone();
                                                        move |_, window, cx| {
                                                            input.update(cx, |st, cx| {
                                                                st.set_value("", window, cx);
                                                                st.focus(window, cx);
                                                            });
                                                        }
                                                    })
                                                    .child(
                                                        icon("icons/circle-x.svg")
                                                            .with_size(px(16.)),
                                                    ),
                                            )
                                        })
                                        .p_0()
                                        .appearance(false),
                                ),
                        )
                        .child(
                            List::new(&list)
                                .with_size(gpui_component::Size::Large)
                                .flex_1()
                                .min_h_0(),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .border_t_1()
                                .border_color(theme.border)
                                .pt(SPACE_SM)
                                // 与输入行的壳同缩进:文字与放大镜/清除钮左右对齐
                                .px(SPACE_SM)
                                .justify_between()
                                .text_size(FONT_LABEL)
                                .text_color(theme.muted_foreground)
                                .child("Scope: all sessions")
                                .child(
                                    h_flex()
                                        .gap(SPACE_MD)
                                        .child("\u{2191}\u{2193} navigate")
                                        .child("\u{21a9} open")
                                        .child("esc close"),
                                ),
                        ),
                )
        });
        let focus_input = self.palette_input.clone();
        cx.defer_in(window, move |_, window, cx| {
            focus_input.update(cx, |st, cx| st.focus(window, cx));
        });
        cx.notify();
    }

    /// ⌘K 输入框事件:文字变化驱动搜索,回车打开选中项
    fn on_palette_input_event(
        &mut self,
        input: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => {
                let q = input.read(cx).value().trim().to_string();
                if self.palette_list.read(cx).delegate().last_query == q {
                    return;
                }
                // 清空(点清除钮/删光)结果已知,同步清态,省掉线程往返
                if q.is_empty() {
                    self._palette_search_task = None;
                    self.palette_list.update(cx, |state, cx| {
                        let d = state.delegate_mut();
                        d.hits = Vec::new();
                        d.degraded = false;
                        d.last_query = String::new();
                        state.set_selected_index(None, window, cx);
                        cx.notify();
                    });
                    return;
                }
                let task = self.palette_list.update(cx, |state, cx| {
                    state.set_selected_index(None, window, cx);
                    state.delegate_mut().perform_search(&q, window, cx)
                });
                // 搜索回填后选中首条并回滚到顶(覆盖旧任务 = 取消过期搜索)
                self._palette_search_task = Some(cx.spawn_in(window, async move |this, cx| {
                    task.await;
                    this.update_in(cx, |this, window, cx| {
                        this.palette_list.update(cx, |state, cx| {
                            let has_hits = !state.delegate().hits.is_empty();
                            state.set_selected_index(has_hits.then(IndexPath::default), window, cx);
                            // scroll_to_item 自带 notify,列表随之重绘
                            state.scroll_to_item(
                                IndexPath::default(),
                                ScrollStrategy::Top,
                                window,
                                cx,
                            );
                        });
                    })
                    .ok();
                }));
            }
            InputEvent::PressEnter { .. } => {
                let row = self
                    .palette_list
                    .read(cx)
                    .selected_index()
                    .map(|i| i.row)
                    .unwrap_or(0);
                self.open_palette_hit(row, window, cx);
            }
            _ => {}
        }
    }

    /// 打开第 row 条搜索命中(回车与鼠标点击共用),定位到命中消息
    fn open_palette_hit(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let hit = self
            .palette_list
            .read(cx)
            .delegate()
            .hits
            .get(row)
            .map(|h| (h.session.key.clone(), h.seq));
        if let Some((key, seq)) = hit {
            window.close_dialog(cx);
            self.open_detail(&key, Some(seq), window, cx);
        }
    }

    /// ⌘K 面板 ↑↓:焦点在输入框,选中态手动挪(clamp 到两端,不循环)。
    /// 按 row 平移——SearchDelegate 单 section,section 恒 0
    fn palette_move(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_list.update(cx, |state, cx| {
            let n = state.delegate().hits.len() as i64;
            if n == 0 {
                return;
            }
            let cur = state.selected_index().map(|ix| ix.row as i64).unwrap_or(-1);
            let next = (cur + delta).clamp(0, n - 1) as usize;
            state.set_selected_index(Some(IndexPath::new(next)), window, cx);
            // scroll_to_selected_item 内部 notify,选中高亮随之重绘
            state.scroll_to_selected_item(window, cx);
        });
    }

    // ---------- 详情 ----------

    fn open_detail(
        &mut self,
        key: &str,
        jump_seq: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(Some(meta)) = self.store.get_session(key) else {
            return;
        };
        self.detail = Some(DetailState {
            meta: meta.clone(),
            transcript: Rc::new(Vec::new()),
            loading: true,
            error: None,
            // Bottom 对齐 = 聊天语义:打开落在最新消息,向上翻历史
            msg_list: gpui::ListState::new(0, gpui::ListAlignment::Bottom, px(512.)),
            expanded_tools: HashSet::new(),
            expanded_thinking: HashSet::new(),
            jump_seq,
            images: Vec::new(),
            zoom: None,
        });
        // 搜索路径:中栏列表同步选中并滚到该会话。
        // 列表点击路径(jump=None)不走——List 点击自带选中,再滚会跳视口
        if jump_seq.is_some() {
            self.sync_list_selection(key, window, cx);
        }
        cx.notify();

        let adapters = self.adapters.clone();
        let task = cx.background_spawn(async move {
            // adapter_for 按文件路径挑实例:自定义 location 的会话必须由
            // 拥有其根的实例解析(gemini/kimi 的 cwd 反查是实例相对侧档)
            let Some(adapter) = adapter_for(&adapters, meta.agent, &meta.file_path) else {
                return Err(format!(
                    "No {} reader is configured for this file.",
                    meta.agent.display_name()
                ));
            };
            let r = SessionFileRef::from_meta(&meta);
            let t = adapter
                .parse_transcript(&r)
                .map_err(|e| format!("Couldn't read this session file: {e}"))?;
            let mut visible: Vec<TranscriptMessage> = t
                .mainline
                .into_iter()
                .filter(|m| {
                    m.kind != MessageKind::Meta
                        && (!m.text.trim().is_empty()
                            || !m.tool_calls.is_empty()
                            || m.thinking.is_some()
                            || !m.images.is_empty()
                            || m.kind == MessageKind::CompactSummary)
                })
                .collect();
            // 字节 move 进 Arc<Image>,消息里那份同时清空——两处各留一份的话
            // 一个图多的会话会白占一倍内存
            let images: Vec<Vec<ImageSlot>> = visible
                .iter_mut()
                .map(|m| {
                    std::mem::take(&mut m.images)
                        .into_iter()
                        .map(|a| match image_format_of(&a.media_type) {
                            Some(f) => {
                                let dims = image_dimensions(&a.bytes);
                                ImageSlot::Ready {
                                    image: Arc::new(gpui::Image::from_bytes(f, a.bytes)),
                                    dims,
                                }
                            }
                            None => ImageSlot::Unsupported(a.media_type.into()),
                        })
                        .collect()
                })
                .collect();
            Ok((meta.key.clone(), visible, images))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _window, cx| {
                if let Some(detail) = &mut this.detail {
                    match result {
                        Ok((key, messages, images)) if key == detail.meta.key => {
                            detail.msg_list = gpui::ListState::new(
                                messages.len(),
                                gpui::ListAlignment::Bottom,
                                px(512.),
                            );
                            // 搜索跳转:seq → 可见消息下标,滚到视口顶。
                            // FTS 命中的行可能被详情过滤(如空文本),用 >= 落到
                            // 其后最近一条;找不到(尾部被滤)则保持默认落底。
                            // jump_seq 归一为落点消息的实际 seq——高亮按精确相等
                            // 渲染,不归一则命中被滤时滚动与高亮指向不同行
                            if let Some(seq) = detail.jump_seq {
                                if let Some(ix) = messages.iter().position(|m| m.seq >= seq) {
                                    detail.jump_seq = Some(messages[ix].seq);
                                    detail.msg_list.scroll_to(gpui::ListOffset {
                                        item_ix: ix,
                                        offset_in_item: px(0.),
                                    });
                                }
                            }
                            detail.transcript = Rc::new(messages);
                            detail.images = images;
                            detail.zoom = None;
                            detail.loading = false;
                        }
                        // key 不匹配 = 已切走,这一轮结果作废
                        Ok(_) => {}
                        Err(reason) => {
                            detail.error = Some(reason.into());
                            detail.loading = false;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 侧栏目的地互斥的唯一写入点:三个筛选字段与"离开 Insights"一起落,
    /// 每个导航行只算自己的目标值——新目的地不必再逐个 listener 补互斥
    fn set_scope(
        &mut self,
        agent: Option<AgentId>,
        project: Option<String>,
        favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected_agent = agent;
        self.selected_project = project;
        self.favorite_only = favorite;
        self.insights_open = false;
        self.refresh(cx);
    }

    /// 清空过滤回 All Sessions 视图(侧栏点击与搜索打开共用;
    /// 已在 All Sessions 时 refresh 幂等,微秒级)
    fn show_all_sessions(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_scope(None, None, false, cx);
    }

    /// 搜索命中打开:侧栏切回 All Sessions(搜索是全库范围,过滤视图下
    /// 命中可能不在列表里),中栏定位选中该会话并滚到可见
    fn sync_list_selection(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_all_sessions(window, cx);
        let row = {
            let delegate = self.list_state.read(cx).delegate();
            delegate
                .sessions
                .iter()
                .position(|s| s.key == key)
                .and_then(|flat| delegate.index_path(flat))
        };
        // 命中排在 limit 之外时 row 为 None:详情页已开,这里只是不滚动
        if let Some(row) = row {
            self.list_state.update(cx, |state, cx| {
                state.set_selected_index(Some(row), window, cx);
                // 组件无 strict-Top:非 Center 策略都是"最小滚动恰好可见",
                // 目标从下方进入会贴底。先把 offset 拉到超底,deferred 消费时
                // 目标位于视口上方,最小滚动分支即把它对齐到视口顶。
                // 耦合 gpui-component 0.5.1 行为;上游 DeferredScrollToItem 的
                // scroll_strict 字段目前写死 false 未被读——它被接通之日,
                // 换成 strict-Top 调用并删掉这行 set_offset
                state.scroll_handle().set_offset(point(px(0.), px(-1e9)));
                state.scroll_to_item(row, ScrollStrategy::Top, window, cx);
            });
        }
    }

    // ---------- 操作 ----------

    /// 后台任务完成 → 推通知的通用桥(do_resume/do_export 共用)
    fn notify_when_done<T: Send + 'static>(
        window: &mut Window,
        cx: &mut Context<Self>,
        task: gpui::Task<T>,
        to_note: impl FnOnce(T) -> Notification + Send + 'static,
    ) {
        cx.spawn_in(window, async move |_this, cx| {
            let result = task.await;
            cx.update(|window, cx| {
                window.push_notification(to_note(result), cx);
            })
            .ok();
        })
        .detach();
    }

    /// remember=false:本次目标是"偏好在这个会话上不可用"时的回退值(见 render
    /// 里的 terminals_for),点它不该把回退值写成新偏好——否则一个 dsh 会话就能
    /// 把用户的 Kooky 偏好冲成 Terminal,再回 Claude 会话也回不去了
    fn do_resume(
        &mut self,
        term: terminal::TerminalApp,
        remember: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = &self.detail else { return };
        if remember {
            self.preferred_terminal = Some(term);
            cx.notify(); // split 按钮左段立即切到本次选择
        }
        let meta = detail.meta.clone();
        let task = cx.background_spawn(async move { terminal::resume_session_in(&meta, term) });
        Self::notify_when_done(window, cx, task, |outcome| {
            if outcome.ok {
                Notification::success(format!("Opened in terminal: {}", outcome.command))
            } else {
                Notification::error(outcome.error.unwrap_or_else(|| "Resume failed".into()))
            }
        });
    }

    fn toggle_favorite(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(detail) = &mut self.detail {
            let v = !detail.meta.favorite;
            let _ = self.store.set_user_data(&detail.meta.key, Some(v), None);
            detail.meta.favorite = v;
            self.refresh(cx);
        }
    }

    fn toggle_pinned(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(detail) = &mut self.detail {
            let v = !detail.meta.pinned;
            let _ = self.store.set_user_data(&detail.meta.key, None, Some(v));
            detail.meta.pinned = v;
            self.refresh(cx);
        }
    }

    fn do_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let meta = detail.meta.clone();
        let adapters = self.adapters.clone();
        let task = cx.background_spawn(async move {
            let adapter = adapter_for(&adapters, meta.agent, &meta.file_path)?;
            // from_meta 对虚拟路径(SQLite 型)自动回退,导出不再依赖真实文件存在
            let r = SessionFileRef::from_meta(&meta);
            let t = adapter.parse_transcript(&r).ok()?;
            let sidechains: Vec<(SidechainInfo, Vec<TranscriptMessage>)> = t
                .sidechains
                .iter()
                .map(|sc| {
                    let msgs = adapter.load_sidechain(&r, &sc.id).unwrap_or_default();
                    (sc.clone(), msgs)
                })
                .collect();
            let md = exporter::to_markdown(&t.meta, &t.mainline, &sidechains);
            let name = exporter::default_file_name(&meta, "md");
            let path = dirs::download_dir()?.join(name);
            std::fs::write(&path, md).ok()?;
            Some(path)
        });
        Self::notify_when_done(window, cx, task, |path| match path {
            Some(p) => Notification::success(format!("Exported to {}", p.display())),
            None => Notification::error("Export failed"),
        });
    }

    /// 执行删除:文件进废纸篓 + 自库 tombstone。trash_paths 可能长阻塞
    /// (契约见 terminal/mod.rs,平台缘由见各实现 doc)——必须离开 UI 线程,
    /// 否则界面在授权框弹出的整段时间里完全冻结。
    fn do_delete(
        &mut self,
        key: String,
        targets: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let trash_key = key.clone();
        let task = cx.background_spawn(async move {
            terminal::trash_paths(&targets).and_then(|()| store.remove_session(&trash_key, true))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    // 等待期间用户可能已翻到别的会话,只在仍停在被删那条时才清空
                    if this.detail.as_ref().is_some_and(|d| d.meta.key == key) {
                        this.detail = None;
                    }
                    window.push_notification(Notification::success(SESSION_TRASHED), cx);
                    // 立刻把它从列表摘掉,不等 watcher 那 800ms 去抖
                    this.refresh(cx);
                }
                Err(e) => {
                    window.push_notification(Notification::error(format!("Delete failed: {e}")), cx)
                }
            })
            .ok();
        })
        .detach();
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let meta = detail.meta.clone();
        // 会话归属哪些磁盘路径(主文件/边车目录)是 adapter 的布局知识
        let targets = adapter_for(&self.adapters, meta.agent, &meta.file_path)
            .map(|a| a.session_paths(&meta))
            .unwrap_or_else(|| vec![meta.file_path.clone()]);
        let entity = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let meta = meta.clone();
            let targets = targets.clone();
            let entity = entity.clone();
            let theme = cx.theme();
            dialog
                .title(
                    div()
                        .text_size(FONT_HEADING)
                        .font_semibold()
                        .child("Delete this session?"),
                )
                .w(px(440.))
                // 破坏性确认:主按钮点名动作并用 danger 形态,不留裸 "OK"。
                // .confirm() 必须显式调用——Dialog 只在设了 footer 时才渲染
                // 按钮行,只挂 on_ok 的弹窗实际无按钮(仅回车可确认)
                .confirm()
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text(MOVE_TO_TRASH)
                        .ok_variant(gpui_component::button::ButtonVariant::Danger),
                )
                .child(
                    v_flex()
                        .gap(SPACE_SM)
                        .text_size(FONT_BODY)
                        .child(TRASH_CONFIRM_BODY)
                        .child(
                            div()
                                .px(SPACE_SM)
                                .py(SPACE_XS)
                                .rounded(theme.radius)
                                .bg(theme.muted)
                                .text_size(FONT_CAPTION)
                                // 等宽走主题 token(Menlo 只有 macOS 有;
                                // Windows 上找不到会静默回落到比例字体的
                                // 系统 UI 字体,与其他路径 chip 不一致)
                                .font_family(theme.mono_font_family.clone())
                                .child(meta.file_path.clone()),
                        )
                        .when(meta.agent == AgentId::Codex, |this| {
                            this.child(
                                div()
                                    .text_size(FONT_CAPTION)
                                    .text_color(theme.muted_foreground)
                                    .child("Only the local file is removed — Codex's own records stay intact."),
                            )
                        }),
                )
                .on_ok(move |_, window, cx| {
                    entity.update(cx, |this, cx| {
                        this.do_delete(meta.key.clone(), targets.clone(), window, cx);
                    });
                    true
                })
        });
    }

    fn context_title(&self) -> String {
        if self.favorite_only {
            return "Starred".to_string();
        }
        if let Some(p) = &self.selected_project {
            // 项目显示名以侧栏列表(store 的 ProjectInfo)为准,不再重推
            return self
                .projects
                .iter()
                .find(|info| info.path == *p)
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "Projects".to_string());
        }
        match self.selected_agent {
            None => "All Sessions".to_string(),
            Some(one) => one.display_name().to_string(),
        }
    }

    // ---------- 渲染 ----------

    fn render_sidebar(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let all_active = self.selected_agent.is_none()
            && self.selected_project.is_none()
            && !self.favorite_only
            && !self.insights_open;
        // 常态沉默,仅刷新中/监听失效时出现;None 时状态栏整行不渲染。
        // 文案在此按 scan 现算,不另存字段——存下来就会有第二个写入点要维护
        let note = if self.scan.scanning {
            Some(match self.scan.total {
                0 => "Refreshing…".to_string(),
                total => format!("Refreshing {}/{}", self.scan.done, total),
            })
        } else {
            self.scan
                .error
                .as_ref()
                .map(|e| format!("Refresh failed: {e}"))
        };
        let status: Option<AnyElement> = if let Some(note) = note {
            Some(
                h_flex()
                    .w_full()
                    .gap(SPACE_SM)
                    .text_color(theme.muted_foreground)
                    .child(
                        icon("icons/refresh-cw.svg")
                            .with_size(px(12.))
                            .flex_shrink_0(),
                    )
                    .child(div().min_w_0().truncate().child(note))
                    .into_any_element(),
            )
        } else if self.watcher.is_none() {
            Some(
                h_flex()
                    .w_full()
                    .gap(SPACE_SM)
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(theme.warning),
                    )
                    .child(div().min_w_0().truncate().child("Live updates off"))
                    .into_any_element(),
            )
        } else {
            None
        };

        // macOS 恒挂 TitleBar(traffic light 占位 + 拖拽区);Linux/Windows 按
        // 运行时装饰状态:系统给了标题栏(Server)就不挂——TitleBar 的非 mac
        // 实现无条件画 min/max/close 三按钮,会与系统标题栏成双套控制;系统
        // 不给(GNOME Wayland 无 SSD 回落 Client)才挂,此时它是唯一的拖拽区
        // 与窗口按钮(按钮图标 window-*.svg 在 assets 注册表,缺了就是隐形
        // 热区)。Windows 走 appears_transparent=false 的原生 caption,装饰
        // 恒报 Server,这里天然不挂。
        let show_titlebar = cfg!(target_os = "macos")
            || matches!(window.window_decorations(), Decorations::Client { .. });
        v_flex()
            .w(SIDEBAR_W)
            .h_full()
            .flex_shrink_0()
            .bg(theme.sidebar)
            // 压平 titlebar 靠 theme.rs 的 title_bar/title_bar_border token；主窗口
            // 使用 44px 高度，与详情顶部行共享同一垂直节奏。
            .when(show_titlebar, |this| {
                this.child(TitleBar::new().h(WINDOW_TITLEBAR_HEIGHT))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .h(WINDOW_TITLEBAR_HEIGHT)
                    .px(SIDEBAR_EDGE)
                    .pt(SPACE_XS)
                    .pb(SPACE_LG)
                    .child(
                        div()
                            .pl(TITLE_INSET)
                            .pr(SIDEBAR_EDGE)
                            .text_size(FONT_HEADING)
                            .font_semibold()
                            .text_color(theme.foreground)
                            .child("Wake"),
                    ),
            )
            .child(
                div().flex_shrink_0().px(SIDEBAR_EDGE).pb(SPACE_MD).child(
                    h_flex().gap(SPACE_SM).child(
                        h_flex()
                            .id("sidebar-search")
                            .flex_1()
                            .min_w_0()
                            .h(ROW_HEIGHT)
                            .px(SIDEBAR_EDGE)
                            .gap(SPACE_SM)
                            .rounded(theme.radius)
                            .cursor_pointer()
                            .bg(theme.secondary)
                            .text_size(FONT_CAPTION)
                            .text_color(theme.muted_foreground)
                            .hover(|s| {
                                s.bg(theme.secondary_hover)
                                    .text_colored(theme.foreground, FONT_CAPTION)
                            })
                            .active(|s| {
                                s.bg(theme.secondary_active)
                                    .text_colored(theme.foreground, FONT_CAPTION)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_search(&ToggleSearch, window, cx)
                            }))
                            .child(icon("icons/search.svg").with_size(px(13.)).flex_shrink_0())
                            // flex_1 + min_w_0 + truncate:空间不足时压这里,
                            // 绝不把右侧刷新按钮挤出侧栏
                            .child(div().flex_1().min_w_0().truncate().child("Search sessions"))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(FONT_LABEL)
                                    .child(search_key_hint()),
                            ),
                    ),
                ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px(SIDEBAR_EDGE)
                    .pb(SPACE_XS)
                    .gap(SPACE_XS)
                    .child(sidebar_row(
                        "all",
                        RowLead::Icon(icon("icons/layers.svg")),
                        "All Sessions",
                        Some(self.agent_counts.iter().map(|(_, n)| n).sum()),
                        all_active,
                        RowLevel::Primary,
                        cx.listener(|this, _, window, cx| {
                            this.show_all_sessions(window, cx);
                        }),
                        cx,
                    ))
                    .child(sidebar_row(
                        "fav",
                        RowLead::Icon(icon("icons/star.svg")),
                        "Starred",
                        if self.starred_count > 0 {
                            Some(self.starred_count)
                        } else {
                            None
                        },
                        self.favorite_only,
                        RowLevel::Primary,
                        cx.listener(|this, _, _window, cx| {
                            // 取消收藏过滤时 agent/project 必已是 None(互斥),
                            // 两个方向都归 set_scope
                            let favorite = !this.favorite_only;
                            this.set_scope(None, None, favorite, cx);
                        }),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SIDEBAR_EDGE)
                    .pt(SPACE_XS)
                    .pb(SPACE_LG)
                    .gap(SPACE_XS)
                    .child(group_header(
                        "agents-header",
                        "Agents",
                        self.agents_collapsed,
                        cx.listener(|this, _, _window, cx| {
                            this.agents_collapsed = !this.agents_collapsed;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .when(!self.agents_collapsed, |this| {
                        this.children(self.agent_counts.iter().map(|(agent, count)| {
                            let agent = *agent;
                            sidebar_row(
                                agent.as_str(),
                                RowLead::Brand(agent.brand_icon(theme.mode.is_dark())),
                                agent.display_name(),
                                Some(*count),
                                self.selected_agent == Some(agent),
                                RowLevel::Sub,
                                cx.listener(move |this, _, _window, cx| {
                                    let next = if this.selected_agent == Some(agent) {
                                        None
                                    } else {
                                        Some(agent)
                                    };
                                    this.set_scope(next, None, false, cx);
                                }),
                                cx,
                            )
                        }))
                    })
                    .child(group_header(
                        "projects-header",
                        "Projects",
                        self.projects_collapsed,
                        cx.listener(|this, _, _window, cx| {
                            this.projects_collapsed = !this.projects_collapsed;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .when(!self.projects_collapsed, |this| {
                        this.children(self.projects.iter().enumerate().map(|(ix, p)| {
                            let path = p.path.clone();
                            sidebar_row(
                                ("proj", ix),
                                RowLead::Icon(icon("icons/folder.svg")),
                                p.name.clone(),
                                Some(p.session_count),
                                self.selected_project.as_deref() == Some(p.path.as_str()),
                                RowLevel::Sub,
                                cx.listener(move |this, _, _window, cx| {
                                    let next = if this.selected_project.as_deref()
                                        == Some(path.as_str())
                                    {
                                        None
                                    } else {
                                        Some(path.clone())
                                    };
                                    this.set_scope(None, next, false, cx);
                                }),
                                cx,
                            )
                        }))
                    }),
            )
            // 底部工具条:次要操作(数据源、刷新)与扫描状态同处一区,与上方
            // 导航行只用一条 border 分隔。按钮透明底、hover 才出色,不跟导航
            // 行的选中态抢注意力;图标-only 元素改 text_color 不丢字号
            .child(
                v_flex()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .when_some(status, |this, status| {
                        this.child(
                            h_flex()
                                .px(SPACE_XL)
                                .pt(SPACE_MD)
                                .text_size(FONT_LABEL)
                                .child(status),
                        )
                    })
                    .child(
                        h_flex()
                            .h(SIDEBAR_FOOTER_ROW_HEIGHT)
                            .px(SIDEBAR_EDGE)
                            .items_center()
                            .justify_end()
                            .gap(SPACE_XS)
                            .child(sidebar_tool_btn(
                                "insights",
                                "Insights",
                                true,
                                // 页面打开时图标点亮 primary(显式设色后不被
                                // hover 的容器 text_color 覆盖)
                                {
                                    let mut ic = icon("icons/chart-column.svg").with_size(px(14.));
                                    if self.insights_open {
                                        ic = ic.text_color(theme.primary);
                                    }
                                    ic.into_any_element()
                                },
                                cx.listener(|this, _, _window, cx| this.toggle_insights(cx)),
                                cx,
                            ))
                            .child(sidebar_tool_btn(
                                "settings",
                                "Settings",
                                true,
                                icon("icons/settings.svg")
                                    .with_size(px(14.))
                                    .into_any_element(),
                                cx.listener(|this, _, _window, cx| this.open_settings(cx)),
                                cx,
                            ))
                            .child(sidebar_tool_btn(
                                "refresh",
                                "Refresh sessions",
                                !self.scan.scanning,
                                if self.scan.scanning {
                                    Spinner::new().small().into_any_element()
                                } else {
                                    icon("icons/refresh-cw.svg")
                                        .with_size(px(14.))
                                        .into_any_element()
                                },
                                cx.listener(|this, _, window, cx| {
                                    this.refresh_sessions(window, cx)
                                }),
                                cx,
                            )),
                    ),
            )
    }

    fn render_session_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let shown = self.list_state.read(cx).delegate().sessions.len();
        let sort_key = self.sort_key;
        let sort_ascending = self.sort_ascending;
        let sort_entity = cx.entity();
        let sort_label = match sort_key {
            SortKey::Updated => "Date updated",
            SortKey::Created => "Date created",
            SortKey::Messages => "Message count",
        };
        let sort_tooltip = format!(
            "Sort by {} · {}",
            sort_label,
            if sort_ascending {
                "Ascending"
            } else {
                "Descending"
            }
        );
        // 与详情工具栏统一：icon-only ghost，常态透明、hover 才出现背景。
        // 当前排序方式由 tooltip 给出
        let sort_menu = Button::new("sort-sessions")
            .ghost()
            .rounded(RADIUS_BUTTON)
            .icon(icon("icons/arrow-up-down.svg").with_size(px(16.)))
            .tooltip(sort_tooltip)
            .dropdown_menu(move |menu, _, _| {
                let mk_key = |label: &'static str, key: SortKey| {
                    let entity = sort_entity.clone();
                    PopupMenuItem::new(label).checked(sort_key == key).on_click(
                        move |_, _window, cx| {
                            entity.update(cx, |this, cx| {
                                this.sort_key = key;
                                this.refresh(cx);
                            });
                        },
                    )
                };
                let mk_dir = |label: &'static str, ascending: bool| {
                    let entity = sort_entity.clone();
                    PopupMenuItem::new(label)
                        .checked(sort_ascending == ascending)
                        .on_click(move |_, _window, cx| {
                            entity.update(cx, |this, cx| {
                                this.sort_ascending = ascending;
                                this.refresh(cx);
                            });
                        })
                };
                menu.min_w(px(180.))
                    .item(mk_key("Date updated", SortKey::Updated))
                    .item(mk_key("Date created", SortKey::Created))
                    .item(mk_key("Message count", SortKey::Messages))
                    .separator()
                    .item(mk_dir("Descending", false))
                    .item(mk_dir("Ascending", true))
            })
            .anchor(Corner::TopRight);
        // 被 limit 截断时要说出来,否则侧栏计数与列表长度互相矛盾
        let truncated = self.total_sessions > shown as i64;
        v_flex()
            .w(STREAM_W)
            .h_full()
            .flex_shrink_0()
            .bg(theme.list)
            .child(
                v_flex()
                    .id("list-header")
                    .w_full()
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .px(SPACE_LG)
                    // 与详情头同一个顶部偏移:两栏标题因此等高。不用固定高度
                    // 居中——中栏只有标题一行、详情有标题+元信息两行,居中之下
                    // 两者必然错开,且「标题对齐」与「组头对齐元信息」不可兼得
                    .pt(SPACE_XL)
                    // 首组与标题的距离 = 这里 + 组头 pt。加在这一侧而不是组头上:
                    // 组头 pt 同时管着组间距,且对所有组头必须一致
                    .pb(SPACE_SM)
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .justify_between()
                            .gap(SPACE_SM)
                            .child(
                                // 角标是带内边距的胶囊,baseline 对齐会坐低半档
                                h_flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap(SPACE_SM)
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(FONT_TITLE)
                                            .font_semibold()
                                            .child(self.context_title()),
                                    )
                                    .when(self.total_sessions > 0, |this| {
                                        this.child(
                                            div().flex_shrink_0().text_size(FONT_LABEL).child(
                                                badge(
                                                    self.total_sessions.to_string(),
                                                    theme.muted,
                                                    theme.muted_foreground,
                                                ),
                                            ),
                                        )
                                    }),
                            )
                            .child(div().flex_shrink_0().pt(px(2.)).child(sort_menu)),
                    ),
            )
            .child(if shown == 0 {
                v_flex()
                    .flex_1()
                    .justify_center()
                    // 首次全量扫描期间列表本来就是空的,不能报"没有匹配项"
                    .child(if self.scan.scanning {
                        empty_state(
                            "icons/loader-circle.svg",
                            px(48.),
                            px(22.),
                            "Indexing your sessions",
                            match self.scan.total {
                                0 => "Looking for session files…".to_string(),
                                total => format!("{} of {total} sessions", self.scan.done),
                            },
                            cx,
                        )
                    } else if let Some(err) = self.scan.error.clone() {
                        empty_state(
                            "icons/circle-x.svg",
                            px(48.),
                            px(22.),
                            "Couldn't index your sessions",
                            err,
                            cx,
                        )
                    } else if self.favorite_only
                        || self.selected_agent.is_some()
                        || self.selected_project.is_some()
                    {
                        empty_state(
                            "icons/inbox.svg",
                            px(48.),
                            px(22.),
                            "No sessions here",
                            "Pick All Sessions in the sidebar to see everything.",
                            cx,
                        )
                    } else {
                        empty_state(
                            "icons/inbox.svg",
                            px(48.),
                            px(22.),
                            "No sessions yet",
                            "Wake found no agent sessions on this Mac.",
                            cx,
                        )
                    })
                    .into_any_element()
            } else {
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(List::new(&self.list_state).flex_1().min_h_0())
                    .when(truncated, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .w_full()
                                .px(SPACE_LG)
                                .py(SPACE_SM)
                                .border_t_1()
                                .border_color(theme.border)
                                .text_size(FONT_LABEL)
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(format!(
                                    "Showing the {shown} most recent of {} sessions",
                                    self.total_sessions
                                )),
                        )
                    })
                    .into_any_element()
            })
    }

    /// 放大预览。铺满窗口、压在对话区之上但在 dialog 层之下——
    /// 它不是模态流程,只是"把这张图看清楚",点背景即走。
    ///
    /// 两处必须这么写:
    /// - `occlude()`:gpui 的命中测试会一路派发给所有边界命中的元素,
    ///   不阻断的话遮罩底下的列表和按钮照样能点到。
    /// - 图片的**大投影**:gpui 没有背景模糊(`blur_radius` 只属于
    ///   box-shadow,`window.blur()` 是焦点失焦),遮罩又压得浅,
    ///   只能靠投影把图从背景里浮起来。
    fn render_image_zoom(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((mi, ii)) = self.detail.as_ref().and_then(|d| d.zoom) else {
            return div().into_any_element();
        };
        let Some(ImageSlot::Ready { image, dims }) = self
            .detail
            .as_ref()
            .and_then(|d| d.images.get(mi))
            .and_then(|v| v.get(ii))
            .cloned()
        else {
            return div().into_any_element();
        };
        let meta_line = {
            let kind = image
                .format
                .mime_type()
                .trim_start_matches("image/")
                .to_uppercase();
            let size = crate::format::human_bytes(image.bytes.len());
            match dims {
                Some((w, h)) => format!("{kind} · {w} × {h} · {size}"),
                None => format!("{kind} · {size}"),
            }
        };
        // 显示尺寸必须由我们自己算:gpui 的 `img()` 只在"宽 Auto + 高为绝对值"
        // 时才按 aspect_ratio 反推另一边,交给 flex 撑的话布局盒会大于实际
        // 绘制的图(gpui 在盒内按 contain 画),描边就贴不住图的边缘。
        // 尺寸读不出来时退回一个保守的方框,总比描一个错位的框好
        let shown = zoom_fit(dims, window.viewport_size());

        // 背景与关闭钮各要一个:cx.listener 返回的闭包不可 Clone
        let close_backdrop = cx.listener(|this: &mut Self, _, _window, cx| {
            if let Some(detail) = &mut this.detail {
                detail.zoom = None;
                cx.notify();
            }
        });
        let close_button = cx.listener(|this: &mut Self, _, _window, cx| {
            if let Some(detail) = &mut this.detail {
                detail.zoom = None;
                cx.notify();
            }
        });
        let copy_image = image.clone();
        let save_image = image.clone();
        // 遮罩上的图标钮:底色透明、hover 才出胶囊。它们坐在**有实底的容器**
        // 里(胶囊 / 关闭钮自己),所以不必像散点那样常驻描边
        let ico = |id: &'static str, path: &'static str| {
            h_flex()
                .id(id)
                .size(px(30.))
                .items_center()
                .justify_center()
                .rounded(RADIUS_BUTTON)
                .hover(|s| s.bg(gpui::white().opacity(0.16)))
                .cursor_pointer()
                .child(
                    icon(path)
                        .size(px(15.))
                        .text_color(gpui::white().opacity(0.82)),
                )
        };
        div()
            .id("image-zoom")
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui::black().opacity(IMAGE_SCRIM))
            // 点背景关闭。图片、胶囊、关闭钮各自吞掉点击,不会误关
            .on_click(close_backdrop)
            .child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(SPACE_XL)
                    .px(px(56.))
                    .py(px(52.))
                    .child(
                        gpui::img(image)
                            .id("image-zoom-fig")
                            .w(shown.width)
                            .h(shown.height)
                            .rounded(SPACE_SM)
                            // 投影给图落地感。不描边:白底截图在 58% 遮罩上
                            // 对比已经够,那圈亮边反而像给图套了个框
                            .shadow(vec![zoom_shadow(px(18.), px(48.), 0.45)])
                            .on_click(|_, _, _| {}),
                    )
                    // 元信息与动作收进一个胶囊:有底、有边、有投影,读作一组
                    .child(
                        h_flex()
                            .id("image-zoom-bar")
                            .flex_shrink_0()
                            .h(px(40.))
                            .pl(SPACE_LG)
                            .pr(SPACE_SM)
                            .gap(SPACE_SM)
                            .items_center()
                            .rounded(px(20.))
                            .bg(crate::theme::ZOOM_PILL_BG)
                            .border_1()
                            .border_color(gpui::white().opacity(0.11))
                            .shadow(vec![zoom_shadow(px(8.), px(26.), 0.35)])
                            .on_click(|_, _, _| {})
                            .child(
                                div()
                                    .text_size(FONT_LABEL)
                                    .text_color(gpui::white().opacity(0.58))
                                    .child(meta_line),
                            )
                            .child(div().w(px(1.)).h(px(16.)).bg(gpui::white().opacity(0.16)))
                            .child(
                                ico("image-copy", "icons/copy.svg")
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Copy image")
                                            .build(window, cx)
                                    })
                                    .on_click(move |_, window, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_image(
                                            &copy_image,
                                        ));
                                        window.push_notification(
                                            Notification::success("Image copied"),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                ico("image-save", "icons/download.svg")
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Save to Downloads")
                                            .build(window, cx)
                                    })
                                    .on_click(move |_, window, cx| {
                                        match save_image_to_downloads(&save_image) {
                                            Some(path) => window.push_notification(
                                                Notification::success(format!(
                                                    "Saved to {}",
                                                    path.display()
                                                )),
                                                cx,
                                            ),
                                            None => window.push_notification(
                                                Notification::error("Couldn't save the image"),
                                                cx,
                                            ),
                                        }
                                    }),
                            ),
                    ),
            )
            // 关闭独立在右上角:它是退出,不是"对这张图做点什么",
            // 与胶囊里的两个动作分开
            .child(
                h_flex()
                    .id("image-zoom-close")
                    .absolute()
                    .top(SPACE_LG)
                    .right(SPACE_LG)
                    .size(px(32.))
                    .items_center()
                    .justify_center()
                    .rounded(px(9.))
                    .bg(gpui::white().opacity(0.11))
                    .hover(|s| s.bg(gpui::white().opacity(0.2)))
                    .cursor_pointer()
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Close").build(window, cx)
                    })
                    .on_click(close_button)
                    .child(
                        icon("icons/x.svg")
                            .size(px(15.))
                            .text_color(gpui::white().opacity(0.85)),
                    ),
            )
            .into_any_element()
    }

    // ---------- 对话区逐消息渲染(设计语言:用户右气泡 / 助手平铺 / 工具折叠簇) ----------

    /// gpui::list 的行渲染。在布局阶段经 entity.update 调用(render 已返回,
    /// lease 已释放,无 double-lease 风险——与 dialog builder 的时机不同)。
    fn render_msg_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 窄窗口下阅读区不足 PROSE_MAX_W,工具卡参数格数要跟着收
        let prose_w =
            (window.viewport_size().width - SIDEBAR_W - STREAM_W - SPACE_MD * 2. - px(20.))
                .clamp(px(220.), PROSE_MAX_W);
        // 减去卡片内边距 26 + chevron 11 + gap 14 + 名称与徽标约 100
        let tool_arg_cells = cells_for((prose_w - px(151.)).max(px(80.)), CELL_PX_MONO);
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        // 尾部要用的 Copy 值提前取出,theme 借用不跨越 inner 构建期的 &mut cx
        let jump_bg = theme.primary.opacity(0.09);
        let jump_radius = theme.radius;
        let img_border = crate::theme::panel_border(dark);
        let img_panel = crate::theme::panel_bg(dark);
        let img_muted = theme.muted_foreground;
        let Some(detail) = &self.detail else {
            return div().into_any_element();
        };
        let total = detail.transcript.len();
        let tools_open = detail.expanded_tools.contains(&ix);
        let think_open = detail.expanded_thinking.contains(&ix);
        let jump_seq = detail.jump_seq;
        // Rc 克隆只加引用计数;逐行借用,避免每帧深拷贝整条消息(text 可达 32KB)
        let transcript = detail.transcript.clone();
        // 本行的图。克隆的是 Arc,不是字节;提前取出让 &self.detail 的借用
        // 在元素构建之前结束(下面要 &mut cx)
        let shots: Vec<ImageSlot> = detail.images.get(ix).cloned().unwrap_or_default();
        let Some(m) = transcript.get(ix) else {
            return div().into_any_element();
        };
        // 只有 thinking、没有回复正文或工具调用的中间事件属于运行日志，
        // 连续铺在阅读视图里会把真正的对话切碎；完整原始记录仍保留在源文件中。
        if matches!(m.role, Role::Assistant)
            && m.text.is_empty()
            && m.tool_calls.is_empty()
            && m.thinking.is_some()
        {
            return div().into_any_element();
        }
        // 搜索跳转的落点消息:淡 primary 底色保持高亮,直到换会话
        let is_jump_target = jump_seq == Some(m.seq);

        let inner: AnyElement = if m.kind == MessageKind::CompactSummary {
            centered_pill("Context compacted", cx).into_any_element()
        } else {
            match m.role {
                // 角色靠形态区分:用户右对齐气泡,助手全宽平铺,不加文字标签
                Role::User => {
                    let has_text = !m.text.trim().is_empty();
                    // 纯图消息把内边距收到 7px,让方格几乎撑满气泡;有文字时
                    // 走常规内边距,图排在文字上方(贴图提问的原始顺序)
                    let solo = !shots.is_empty() && !has_text;
                    let mut bubble = div()
                        .max_w(BUBBLE_MAX_W)
                        .min_w_0()
                        .rounded(RADIUS_BUBBLE)
                        .bg(crate::theme::bubble_bg(dark))
                        .text_size(FONT_MSG_USER)
                        .line_height(relative(LINE_HEIGHT_BUBBLE));
                    bubble = if solo {
                        bubble.p(px(7.))
                    } else {
                        bubble.px(px(17.)).py(px(11.))
                    };
                    if !shots.is_empty() {
                        let strip =
                            image_strip(ix, &shots, img_border, img_panel, img_muted, cx.entity());
                        bubble = bubble.child(if has_text { strip.mb(px(9.)) } else { strip });
                    }
                    if has_text {
                        bubble = bubble.child(markdown_body(
                            format!("dmsg-{}", m.seq).into(),
                            m.text.clone(),
                            FONT_MSG_USER,
                            dark,
                            window,
                            cx,
                        ));
                    }
                    h_flex()
                        .w_full()
                        .justify_end()
                        .child(bubble)
                        .into_any_element()
                }
                Role::Assistant => {
                    // thinking 卡 / 正文 / 工具卡之间的呼吸
                    let mut col = v_flex().w_full().min_w_0().gap(px(12.));
                    if let Some(th) = &m.thinking {
                        col = col.child(think_panel(
                            ix,
                            th,
                            think_open,
                            cx.listener(move |this, _, _window, cx| {
                                if let Some(detail) = &mut this.detail {
                                    if !detail.expanded_thinking.insert(ix) {
                                        detail.expanded_thinking.remove(&ix);
                                    }
                                    detail.msg_list.splice(ix..ix + 1, 1);
                                }
                                cx.notify();
                            }),
                            cx,
                        ));
                    }
                    if !m.text.is_empty() {
                        col = col.child(
                            div()
                                .text_size(FONT_MSG_BODY)
                                .line_height(relative(LINE_HEIGHT_PROSE))
                                .child(markdown_message(
                                    m.seq,
                                    &m.text,
                                    FONT_MSG_BODY,
                                    dark,
                                    window,
                                    cx,
                                )),
                        );
                    }
                    if !m.tool_calls.is_empty() {
                        col = col.child(tool_cluster(
                            ix,
                            &m.tool_calls,
                            tool_arg_cells,
                            tools_open,
                            cx.listener(move |this, _, _window, cx| {
                                if let Some(detail) = &mut this.detail {
                                    if !detail.expanded_tools.insert(ix) {
                                        detail.expanded_tools.remove(&ix);
                                    }
                                    // 行高随展开变化,让 list 重测该行
                                    detail.msg_list.splice(ix..ix + 1, 1);
                                }
                                cx.notify();
                            }),
                            cx,
                        ));
                    }
                    col.into_any_element()
                }
                Role::System => centered_pill(one_line(&m.text, 120), cx).into_any_element(),
            }
        };

        div()
            .w_full()
            .flex()
            .justify_center()
            .px(px(10.))
            .py(px(15.))
            .when(ix == 0, |d| d.pt(SPACE_XXL))
            .when(ix + 1 == total, |d| d.pb(SPACE_XXL))
            .child(
                div()
                    .w_full()
                    .max_w(PROSE_MAX_W)
                    .min_w_0()
                    // 淡 primary 底:标记搜索命中落点(尾部命中不滚动,全靠它识别)。
                    // 负 margin + 等量 padding:背景向外扩出呼吸边,内容原位不推挤,
                    // 与相邻消息的对齐和行距都不变
                    .when(is_jump_target, |d| {
                        d.rounded(jump_radius)
                            .bg(jump_bg)
                            .mx(-SPACE_SM)
                            .px(SPACE_SM)
                            .my(-SPACE_XS)
                            .py(SPACE_XS)
                    })
                    .child(inner),
            )
            .into_any_element()
    }

    // ---------------- Insights ----------------

    /// Insights 整页(替换中栏+右栏)。头部沿用中栏 88px 标题节奏兼窗口
    /// 拖拽区;内容 720px 阅读宽居中,区块只用留白与组头分隔,不做卡片墙
    fn render_insights(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        // 副标题只说 "Since {首会话月份}"(用户定稿);拿不到时间时回落默认句
        let subtitle: SharedString = match &self.insights {
            Some(d) if d.sessions > 0 => match month_year(d.first_ts) {
                my if my.is_empty() => "Your coding agent activity".into(),
                my => format!("Since {my}").into(),
            },
            _ => "Your coding agent activity".into(),
        };

        let body: AnyElement = match &self.insights {
            Some(d) if d.sessions > 0 => self.render_insights_content(d, cx),
            _ if self.insights_loading => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
            _ => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state_card(
                    "icons/chart-column.svg",
                    px(58.),
                    px(24.),
                    "No activity yet",
                    "Refresh sessions to see your activity here.",
                    cx,
                ))
                .into_any_element(),
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme.background)
            .child(
                v_flex()
                    .id("insights-header")
                    .w_full()
                    .h(LIBRARY_IDENTITY_HEIGHT)
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .px(SPACE_XXL)
                    .justify_center()
                    .child(
                        v_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(FONT_TITLE)
                                    .font_semibold()
                                    .child("Insights"),
                            )
                            .child(
                                div()
                                    .text_size(FONT_LABEL)
                                    .text_color(theme.muted_foreground)
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    fn render_insights_content(&self, d: &InsightsData, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();

        // ---- 概览大数字行 ----
        let stat = |value: String, label: &'static str| {
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(FONT_DISPLAY)
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(FONT_CAPTION)
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
        };
        // 序:Sessions / Tokens / Prompts / Agents / Projects / Active days
        // (用户钉的)
        let overview = h_flex()
            .gap(SPACE_XXL)
            .child(stat(thousands(d.sessions), "Sessions"))
            .when(d.tokens > 0, |row| {
                row.child(stat(fmt_tokens(Some(d.tokens)), "Tokens"))
            })
            .child(stat(thousands(d.prompts), "Prompts"))
            .child(stat(thousands(d.agents.len() as i64), "Agents"))
            .child(stat(thousands(d.project_count), "Projects"))
            .child(stat(thousands(d.active_days()), "Active days"));

        // ---- 三个榜单的行首/名称(闭包只捕获 Copy 的色值) ----
        let dark = theme.mode.is_dark();
        let muted = theme.muted_foreground;
        let agent_head = move |u: &UsageTally| match AgentId::from_str(&u.name) {
            Some(agent) => (
                Some(img(agent.brand_icon(dark)).size(px(15.)).into_any_element()),
                agent.display_name().into(),
            ),
            // 库里出现未知 agent_id(降级防御):无图标裸名,比整行消失诚实
            None => (None, u.name.clone().into()),
        };
        let project_head = move |u: &UsageTally| {
            (
                Some(
                    icon("icons/folder.svg")
                        .with_size(px(14.))
                        .text_color(muted)
                        .into_any_element(),
                ),
                u.name.clone().into(),
            )
        };
        let model_head = |u: &UsageTally| (None, u.name.clone().into());

        div()
            .id("insights-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div().w_full().flex().justify_center().px(SPACE_XXL).child(
                    v_flex()
                        .w_full()
                        .max_w(px(720.))
                        .pt(SPACE_SM)
                        .pb(px(40.))
                        .gap(px(32.))
                        .child(overview)
                        .child(
                            v_flex()
                                .gap(SPACE_MD)
                                .child(switch_section_head(
                                    "Activity",
                                    Some("Prompts you sent, day by day".into()),
                                    None,
                                    cx,
                                ))
                                .child(render_heatmap(d, cx)),
                        )
                        .child(self.render_distribution_section(d, cx))
                        .child(self.render_usage_section(
                            UsageBoard::Agents,
                            &d.agents,
                            agent_head,
                            cx,
                        ))
                        .when(!d.projects.is_empty(), |col| {
                            col.child(self.render_usage_section(
                                UsageBoard::Projects,
                                &d.projects,
                                project_head,
                                cx,
                            ))
                        })
                        .when(!d.models.is_empty(), |col| {
                            col.child(self.render_usage_section(
                                UsageBoard::Models,
                                &d.models,
                                model_head,
                                cx,
                            ))
                        }),
                ),
            )
            .into_any_element()
    }

    /// 分布区块:标题右侧 ‹ › 在 hour/weekday/month 三个维度间循环切换
    fn render_distribution_section(&self, d: &InsightsData, cx: &Context<Self>) -> AnyElement {
        let range = self.insights_range;
        let values: &[i64] = match range {
            InsightsRange::Hour => &d.hourly,
            InsightsRange::Weekday => &d.weekday,
            InsightsRange::Month => &d.monthly,
        };
        // 峰值只算这一次:caption 点名的与图里高亮的必须是同一根柱
        let (peak, peak_n) = values
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| **n)
            .map(|(i, n)| (i, *n))
            .unwrap_or((0, 0));
        let arrows = insight_arrows(
            "dist-arrow",
            None,
            cx.listener(move |this, _, _window, cx| {
                this.insights_range = this.insights_range.prev();
                cx.notify();
            }),
            cx.listener(move |this, _, _window, cx| {
                this.insights_range = this.insights_range.next();
                cx.notify();
            }),
            cx,
        );
        v_flex()
            .gap(SPACE_MD)
            .child(switch_section_head(
                range.title(),
                Some(dist_caption(range, peak, peak_n).into()),
                Some(arrows.into_any_element()),
                cx,
            ))
            .child(render_distribution(range, values, peak, cx))
            .into_any_element()
    }

    /// 榜单区块(Agents/Projects/Models 同构):‹ 度量名 › 循环切换,行按
    /// 当前度量降序重排再截断 top-N。可用档位就是一个切片:组内无人报
    /// token 时 Tokens 不在其中,归一、循环、行过滤都从这一个事实推出
    fn render_usage_section(
        &self,
        board: UsageBoard,
        rows: &[UsageTally],
        row_head: impl Fn(&UsageTally) -> (Option<AnyElement>, SharedString),
        cx: &Context<Self>,
    ) -> AnyElement {
        use InsightsMetric::*;
        let has_tokens = rows.iter().any(|u| u.tokens > 0);
        // 循环序与概览行同:Sessions / Tokens / Prompts(用户钉的)
        let avail: &[InsightsMetric] = if has_tokens {
            &[Sessions, Tokens, Prompts]
        } else {
            &[Sessions, Prompts]
        };
        let slot = board as usize;
        // position 找不到 = 存的档位已不可用(如数据刷新后 token 清零),
        // 落回首档;循环即可用档位上的环形下标
        let i = avail
            .iter()
            .position(|m| *m == self.insights_metrics[slot])
            .unwrap_or(0);
        let metric = avail[i];
        let to_prev = avail[(i + avail.len() - 1) % avail.len()];
        let to_next = avail[(i + 1) % avail.len()];
        let arrows = insight_arrows(
            board.arrow_id(),
            Some(metric.caption().into()),
            cx.listener(move |this, _, _window, cx| {
                this.insights_metrics[slot] = to_prev;
                cx.notify();
            }),
            cx.listener(move |this, _, _window, cx| {
                this.insights_metrics[slot] = to_next;
                cx.notify();
            }),
            cx,
        );
        let mut sorted: Vec<&UsageTally> = rows
            .iter()
            // Tokens 档只列报了用量的组:空条不是"用了 0",是没数据
            .filter(|u| metric != Tokens || u.tokens > 0)
            .collect();
        // stable sort:平局保持 SQL 的 sessions desc + 名称序
        sorted.sort_by_key(|u| std::cmp::Reverse(metric.value(u)));
        sorted.truncate(board.limit());
        let max = sorted.iter().map(|u| metric.value(u)).max().unwrap_or(1);
        v_flex()
            .gap(SPACE_MD)
            .child(switch_section_head(
                board.title(),
                None,
                Some(arrows.into_any_element()),
                cx,
            ))
            .children(sorted.into_iter().map(|u| {
                let (lead, label) = row_head(u);
                usage_bar_row(
                    lead,
                    label,
                    metric.display(u).into(),
                    metric.value(u),
                    max,
                    board.name_w(),
                    cx,
                )
            }))
            .into_any_element()
    }

    fn render_detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(detail) = &self.detail else {
            return v_flex()
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .bg(theme.background)
                .child(empty_state_card(
                    "icons/message-square.svg",
                    px(58.),
                    px(26.),
                    "No session selected",
                    format!(
                        "Pick one from the list, or press {} to search.",
                        search_key_hint()
                    ),
                    cx,
                ))
                .into_any_element();
        };
        let meta = &detail.meta;
        // 窗口宽 − 侧栏 − 会话流 − 头部内边距 − 操作区,每帧重算
        let title_w = (window.viewport_size().width
            - SIDEBAR_W
            - STREAM_W
            - SPACE_LG * 2.
            - DETAIL_ACTIONS_W)
            .max(px(140.));
        let session_id = meta.id.clone();
        let export_entity = cx.entity();
        let reveal_entity = export_entity.clone();
        let delete_entity = export_entity.clone();
        let more_menu = Button::new("more-actions")
            .ghost()
            .rounded(RADIUS_BUTTON)
            .icon(icon("icons/more-horizontal.svg").with_size(px(16.)))
            .dropdown_menu(move |menu, _, _| {
                let export_entity = export_entity.clone();
                let reveal_entity = reveal_entity.clone();
                let delete_entity = delete_entity.clone();
                menu.min_w(px(210.))
                    .item(
                        PopupMenuItem::new(" Export as Markdown")
                            .icon(icon("icons/download.svg").with_size(px(15.)))
                            .on_click(move |_, window, cx| {
                                export_entity.update(cx, |this, cx| {
                                    this.do_export(window, cx);
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new(format!(" {REVEAL_IN_FM}"))
                            .icon(icon("icons/folder.svg").with_size(px(15.)))
                            .on_click(move |_, _, cx| {
                                reveal_entity.update(cx, |this, _| {
                                    if let Some(detail) = &this.detail {
                                        terminal::reveal_in_file_manager(&detail.meta.file_path);
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new(" Copy Session ID")
                            .icon(icon("icons/copy.svg").with_size(px(15.)))
                            .on_click({
                                let id = session_id.clone();
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                                }
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(format!(" {MOVE_TO_TRASH}"))
                            .icon(icon("icons/trash-2.svg").with_size(px(15.)))
                            .on_click(move |_, window, cx| {
                                delete_entity.update(cx, |this, cx| {
                                    this.confirm_delete(window, cx);
                                });
                            }),
                    )
            })
            .anchor(Corner::TopRight);

        let mut detail_facts: Vec<String> = Vec::new();
        if meta.message_count > 0 {
            detail_facts.push(format!("{} messages", meta.message_count));
        }
        if let Some(tokens) = meta.tokens_used {
            detail_facts.push(format!("{} tokens", fmt_tokens(Some(tokens))));
        }

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme.background)
            .child(
                v_flex()
                    .id("detail-header")
                    .w_full()
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .px(SPACE_LG)
                    .pt(SPACE_XL)
                    .pb(SPACE_MD)
                    // 两层:标题行(含操作区)+ 单条元信息带。gap 与会话流「header
                    // 的 pb + 组头 pt」等值,首组组头因此与这条元信息带齐平
                    .gap(SPACE_LG)
                    .child(
                        h_flex()
                            .gap(SPACE_LG)
                            .items_center()
                            .child(
                                // 单行截断。格数由 title_w 反算而非写死,否则
                                // 拖拽窗口变宽后截断长度不会补齐;全文进 tooltip
                                div()
                                    .id("detail-title")
                                    .w(title_w)
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(FONT_TITLE)
                                    .font_semibold()
                                    .tooltip({
                                        let full: SharedString = meta.title.clone().into();
                                        move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(full.clone())
                                                .build(window, cx)
                                        }
                                    })
                                    .child(clip_display(
                                        &meta.title,
                                        cells_for(title_w, CELL_PX_TITLE),
                                    )),
                            )
                            .child(div().flex_1().min_w_0())
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap(SPACE_XS)
                                    .child({
                                        // Open In split 按钮(Codex/kooky 风):左段 = 上次
                                        // 目标的应用图标,点击直开;右段 chevron 展开列表。
                                        // 目标列表按 agent 过滤(Kooky 深链不认 dsh);
                                        // 偏好目标不在列表时(如 dsh 会话 + 偏好 Kooky)回退首项
                                        let terms = terminal::terminals_for(meta.agent);
                                        let current = self
                                            .preferred_terminal
                                            .filter(|t| terms.contains(t))
                                            .or_else(|| terms.first().copied());
                                        // 偏好已存在时 current 要么就是它、要么是回退值,
                                        // 两种情况点左段都不该改写偏好(见 do_resume)
                                        let remember_current = self.preferred_terminal.is_none();
                                        let current_icon = current
                                            .and_then(|t| self.terminal_icons.get(t.id()).cloned());
                                        let term_items: Vec<(terminal::TerminalApp, Option<PathBuf>)> =
                                            terms
                                                .iter()
                                                .map(|t| {
                                                    (*t, self.terminal_icons.get(t.id()).cloned())
                                                })
                                                .collect();
                                        let menu_entity = cx.entity();
                                        // 无常显分隔线,hover 分段高亮暗示两段(Codex 同款);
                                        // 右段 Button 用 custom variant 与左段 hover 完全一致
                                        h_flex()
                                            .h(px(28.))
                                            .rounded(RADIUS_BUTTON)
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.secondary)
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .id("open-in-main")
                                                    .h_full()
                                                    .px(px(7.))
                                                    .flex()
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme.secondary_hover))
                                                    .active(|s| s.bg(theme.secondary_active))
                                                    .child(match &current_icon {
                                                        Some(p) => img(p.clone())
                                                            .size(px(14.))
                                                            .into_any_element(),
                                                        None => icon("icons/terminal.svg")
                                                            .with_size(px(13.))
                                                            .text_color(theme.secondary_foreground)
                                                            .into_any_element(),
                                                    })
                                                    .tooltip({
                                                        let label: SharedString = match current {
                                                            Some(t) => format!("Open this session in {}", t.display_name()).into(),
                                                            None => "Open this session".into(),
                                                        };
                                                        move |window, cx| {
                                                            gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                                                        }
                                                    })
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        if let Some(term) = current {
                                                            this.do_resume(term, remember_current, window, cx);
                                                        } else {
                                                            // 空列表在 macOS 不可能(Terminal.app 恒在),
                                                            // Windows/Linux 上 PATH 被启动器改写时会发生
                                                            // ——静默无操作是死按钮,至少说一声为什么
                                                            window.push_notification(
                                                                Notification::warning(
                                                                    "No terminal application found on PATH",
                                                                ),
                                                                cx,
                                                            );
                                                        }
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .w(px(1.))
                                                    .h_full()
                                                    .flex_shrink_0()
                                                    .bg(theme.border),
                                            )
                                            .child(
                                                Button::new("open-in-more")
                                                    .custom(
                                                        ButtonCustomVariant::new(cx)
                                                            .foreground(theme.muted_foreground)
                                                            .hover(theme.secondary_hover)
                                                            .active(theme.secondary_active),
                                                    )
                                                    .rounded(px(0.))
                                                    .h(px(26.))
                                                    .w(px(22.))
                                                    .icon(
                                                        icon("icons/chevron-down.svg")
                                                            .with_size(px(12.)),
                                                    )
                                                    .tooltip("Open this session in…")
                                                    .dropdown_menu(move |menu, _, _| {
                                                        let mut menu = menu.min_w(px(170.));
                                                        for (term, icon_path) in term_items.clone() {
                                                            let entity = menu_entity.clone();
                                                            menu = menu.item(
                                                                PopupMenuItem::element(move |_, _| {
                                                                    h_flex()
                                                                        .gap(SPACE_SM)
                                                                        .items_center()
                                                                        .child(match &icon_path {
                                                                            Some(p) => img(p.clone())
                                                                                .size(px(16.))
                                                                                .into_any_element(),
                                                                            None => icon("icons/terminal.svg")
                                                                                .with_size(px(15.))
                                                                                .into_any_element(),
                                                                        })
                                                                        .child(term.display_name())
                                                                })
                                                                .on_click(move |_, window, cx| {
                                                                    entity.update(cx, |this, cx| {
                                                                        this.do_resume(term, true, window, cx);
                                                                    });
                                                                }),
                                                            );
                                                        }
                                                        menu
                                                    })
                                                    .anchor(Corner::TopRight),
                                            )
                                    })
                                    .child(tool_btn(
                                        "fav",
                                        "icons/star.svg",
                                        "icons/star-filled.svg",
                                        rgb(crate::theme::STAR_YELLOW).into(),
                                        if meta.favorite {
                                            "Unstar"
                                        } else {
                                            "Star"
                                        },
                                        meta.favorite,
                                        cx.listener(|this, _, window, cx| {
                                            this.toggle_favorite(window, cx)
                                        }),
                                    ))
                                    .child(tool_btn(
                                        "pin",
                                        "icons/pin.svg",
                                        "icons/pin-filled.svg",
                                        theme.primary,
                                        if meta.pinned {
                                            "Unpin"
                                        } else {
                                            "Pin"
                                        },
                                        meta.pinned,
                                        cx.listener(|this, _, window, cx| {
                                            this.toggle_pinned(window, cx)
                                        }),
                                    ))
                                    .child(more_menu),
                            ),
                    )
                    .child({
                        // 单条元信息带:身份 · 位置 · 规模 · 时间,同字号同色。
                        // 完整项目路径与精确时间进 tooltip;JSONL 文件路径不再
                        // 占位(中段是 UUID,Reveal 入口在 `…` 菜单里)
                        let mut stats: Vec<String> = Vec::new();
                        if meta.message_count > 0 {
                            stats.push(format!("{} messages", meta.message_count));
                        }
                        if let Some(tokens) = meta.tokens_used {
                            stats.push(format!("{} tokens", fmt_tokens(Some(tokens))));
                        }
                        if meta.updated_at > 0 {
                            stats.push(format!("Updated {}", relative_time(meta.updated_at)));
                        }
                        let exact_time: SharedString = {
                            let mut parts: Vec<String> = Vec::new();
                            if meta.created_at > 0 {
                                parts.push(format!("Created {}", abs_date(meta.created_at)));
                            }
                            if meta.updated_at > 0 {
                                parts.push(format!("Updated {}", abs_date(meta.updated_at)));
                            }
                            parts.join("\n").into()
                        };
                        let project_path: SharedString = if meta.project_path.is_empty() {
                            "Unknown project".into()
                        } else {
                            meta.project_path.clone().into()
                        };
                        let reveal_path = meta.file_path.clone();
                        h_flex()
                            .flex_wrap()
                            .gap(px(7.))
                            .items_center()
                            .text_size(FONT_LABEL)
                            .text_color(theme.muted_foreground)
                            .child(
                                img(meta.agent.brand_icon(theme.mode.is_dark()))
                                    .size(px(13.))
                                    .flex_shrink_0(),
                            )
                            .child(div().flex_shrink_0().child(meta.agent.display_name()))
                            .child(meta_sep(theme.muted_foreground))
                            // hover 看全路径,点击 Reveal
                            .child(
                                div()
                                    .id("detail-project")
                                    .min_w_0()
                                    .truncate()
                                    .cursor_pointer()
                                    .hover(|s| s.text_colored(theme.foreground, FONT_LABEL))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            project_path.clone(),
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(move |_, _, _| {
                                        terminal::reveal_in_file_manager(&reveal_path);
                                    })
                                    .child(clip_display(&meta.project_name, 34)),
                            )
                            // detached HEAD 下透传的 "HEAD" 对用户零信息量
                            .when_some(
                                meta.git_branch.clone().filter(|b| {
                                    let b = b.trim();
                                    !b.is_empty() && b != "HEAD" && b != "detached"
                                }),
                                |this, branch| {
                                    this.child(meta_sep(theme.muted_foreground)).child(
                                        h_flex()
                                            .min_w_0()
                                            .gap(SPACE_XS)
                                            .child(
                                                icon("icons/git-branch.svg")
                                                    .with_size(px(11.))
                                                    .flex_shrink_0(),
                                            )
                                            .child(div().min_w_0().truncate().child(branch)),
                                    )
                                },
                            )
                            // model 用同级 muted 文字而非徽标:品牌色只表达
                            // agent 身份,不该套在所有 agent 的 model 上
                            .when_some(meta.model.clone(), |this, model| {
                                this.child(meta_sep(theme.muted_foreground))
                                    .child(div().flex_shrink_0().child(model))
                            })
                            // source 是枚举值不是自由文本,做成徽标才读得出
                            .when_some(
                                meta.source.clone().filter(|s| !s.is_empty()),
                                |this, source| {
                                    // opencode2 是版本代际标记而非发起平台,
                                    // 用 primary 蓝与 via 徽章(绿)区分
                                    let color = if source == "opencode2" {
                                        theme.primary
                                    } else {
                                        theme.success
                                    };
                                    this.child(outline_badge(source, color))
                                },
                            )
                            .when(!stats.is_empty(), |this| {
                                this.child(meta_sep(theme.muted_foreground)).child(
                                    div()
                                        .id("detail-stats")
                                        .min_w_0()
                                        .truncate()
                                        .when(!exact_time.is_empty(), |el| {
                                            el.tooltip(move |window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    exact_time.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                        })
                                        .child(stats.join(" · ")),
                                )
                            })
                    }),
            )
            .child(if detail.loading {
                h_flex()
                    .flex_1()
                    .bg(theme.popover)
                    .items_center()
                    .justify_center()
                    .gap(SPACE_SM)
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().small())
                    .child(div().text_size(FONT_BODY).child("Loading session…"))
                    .into_any_element()
            } else if let Some(reason) = detail.error.clone() {
                let reveal_path = detail.meta.file_path.clone();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .px(SPACE_MD)
                    .pb(SPACE_MD)
                    .child(
                        v_flex()
                            .w(px(380.))
                            .p(SPACE_XXL)
                            .gap(SPACE_MD)
                            .items_center()
                            .rounded(theme.radius_lg)
                            .bg(theme.popover)
                            .child(empty_state(
                                "icons/circle-x.svg",
                                px(58.),
                                px(26.),
                                "Couldn't open this session",
                                reason,
                                cx,
                            ))
                            .child(
                                Button::new("detail-error-reveal")
                                    .outline()
                                    .small()
                                    .rounded(RADIUS_BUTTON)
                                    .icon(icon("icons/folder.svg").with_size(px(13.)))
                                    .label(REVEAL_IN_FM)
                                    .on_click(move |_, _, _| {
                                        terminal::reveal_in_file_manager(&reveal_path);
                                    }),
                            ),
                    )
                    .into_any_element()
            } else {
                let entity = cx.entity().downgrade();
                div()
                    .flex_1()
                    .min_h_0()
                    .px(SPACE_MD)
                    .pb(SPACE_MD)
                    .child(
                        div()
                            .size_full()
                            .rounded(theme.radius_lg)
                            .bg(theme.popover)
                            .relative()
                            .child(
                        gpui::list(detail.msg_list.clone(), move |ix, window, cx| {
                            entity
                                .upgrade()
                                .map(|e| {
                                    e.update(cx, |this, cx| this.render_msg_row(ix, window, cx))
                                })
                                .unwrap_or_else(|| div().into_any_element())
                        })
                                .size_full(),
                            )
                            .vertical_scrollbar(&detail.msg_list),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}

/// 可折叠分组头:chevron + 文字,点击切换
fn group_header(
    id: &'static str,
    text: &'static str,
    collapsed: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &Context<Workbench>,
) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .flex_shrink_0()
        .pl(GROUP_HEAD_INSET)
        .pr(SIDEBAR_EDGE)
        .pt(SPACE_MD)
        .pb(SPACE_XS)
        .cursor_pointer()
        .active(|s| s.opacity(0.7))
        .on_click(on_click)
        .child(
            h_flex()
                .gap(SPACE_XS)
                // 与主导航行同字号同字重(FONT_BODY / 常规),仅靠 muted 色
                // 与"无行首图标"区分——加粗会让组头压过它统辖的行
                .text_size(FONT_BODY)
                .text_color(theme.muted_foreground)
                .hover(|s| s.text_colored(theme.foreground, FONT_BODY))
                .child(text)
                .child(
                    icon("icons/chevron-right.svg")
                        .with_size(px(13.))
                        .when(!collapsed, |ic| {
                            ic.rotate(gpui::Radians(std::f32::consts::FRAC_PI_2))
                        }),
                ),
        )
}

/// 侧栏行层级:Primary=固定主导航(32px/FONT_BODY),Sub=分组展开项(26px/FONT_CAPTION)。
/// 行首元素一律对齐同一条中轴(见 ui.rs LEAD_BOX),子级不再缩进——
/// 行高与字号是仅剩的层级来源,禁止把子级行改回主导航尺度。
#[derive(Clone, Copy, PartialEq)]
enum RowLevel {
    Primary,
    Sub,
}

/// 侧栏行首元素——每行必须有一个,槽位定宽,保证同组文字起点对齐。
/// Lucide 图标随选中态着色;品牌 PNG 保留原色不着色。
enum RowLead {
    Icon(Icon),
    /// Agent 品牌图标,取 `AgentId::brand_icon()`
    Brand(&'static str),
}

// ---------------- Insights 附件 ----------------

/// 可切换区块共用的 ‹ › ghost 按钮组。label 显示在两键中间(当前档位名),
/// 定宽居中让按钮位置不随文本长短跳动;分布图的档位名就是区块标题,传 None
fn insight_arrows(
    id: &'static str,
    label: Option<SharedString>,
    on_prev: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_next: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_shrink_0()
        .gap(SPACE_XS)
        .child(
            Button::new((id, 0usize))
                .ghost()
                .rounded(RADIUS_BUTTON)
                .icon(icon("icons/chevron-left.svg").with_size(px(14.)))
                .tooltip("Previous view")
                .on_click(on_prev),
        )
        .when_some(label, |row, label| {
            row.child(
                div()
                    .w(px(64.))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .text_size(FONT_CAPTION)
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
        })
        .child(
            Button::new((id, 1usize))
                .ghost()
                .rounded(RADIUS_BUTTON)
                .icon(icon("icons/chevron-right.svg").with_size(px(14.)))
                .tooltip("Next view")
                .on_click(on_next),
        )
}

/// Insights 区块头:标题 + 可选 caption + 可选右上角切换按钮组。
/// caption 有则双行(按钮对齐首行),无则单行居中对齐
fn switch_section_head(
    title: &'static str,
    caption: Option<SharedString>,
    arrows: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let two_line = caption.is_some();
    h_flex()
        .justify_between()
        .map(|head| {
            if two_line {
                head.items_start()
            } else {
                head.items_center()
            }
        })
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(FONT_BODY)
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(title),
                )
                .when_some(caption, |head, caption| {
                    head.child(
                        div()
                            .text_size(FONT_CAPTION)
                            .text_color(theme.muted_foreground)
                            .child(caption),
                    )
                }),
        )
        .when_some(arrows, |head, arrows| head.child(arrows))
}

/// 榜单行:行首 + 名称 + 轨道条 + 计数。value_text 与 count 分开传:
/// Tokens 档显示 "1.2M" 缩写,条仍按原值归一
fn usage_bar_row(
    lead: Option<AnyElement>,
    label: SharedString,
    value_text: SharedString,
    count: i64,
    max: i64,
    name_w: Pixels,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let frac = (count as f32 / max.max(1) as f32).clamp(0., 1.);
    h_flex()
        .h(SPACE_XXL)
        .gap(SPACE_SM)
        .items_center()
        .when_some(lead, |row, lead| {
            row.child(
                div()
                    .w(px(15.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(lead),
            )
        })
        .child(
            div()
                .w(name_w)
                .flex_shrink_0()
                .min_w_0()
                .text_size(FONT_CAPTION)
                .text_color(theme.foreground)
                .truncate()
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .h(px(6.))
                .rounded_full()
                .bg(theme.muted)
                .child(
                    div()
                        .h_full()
                        .w(relative(frac))
                        .rounded_full()
                        .bg(theme.primary),
                ),
        )
        .child(
            div()
                .w(px(56.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_size(FONT_LABEL)
                .text_color(theme.muted_foreground)
                .child(value_text),
        )
}

fn prompts_label(n: i64) -> String {
    match n {
        0 => "No prompts".into(),
        1 => "1 prompt".into(),
        n => format!("{} prompts", thousands(n)),
    }
}

/// 0–23 时 → "12 AM" / "2 PM" 形制(与 smart_time 的 12 小时制一致)
fn hour_label(h: usize) -> String {
    match h % 24 {
        0 => "12 AM".into(),
        12 => "12 PM".into(),
        h if h < 12 => format!("{h} AM"),
        h => format!("{} PM", h - 12),
    }
}

const DOW_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DOW_PLURAL: [&str; 7] = [
    "Mondays",
    "Tuesdays",
    "Wednesdays",
    "Thursdays",
    "Fridays",
    "Saturdays",
    "Sundays",
];
const MONTH_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTH_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// peak 由 render_distribution_section 算一次传入——caption 点名的与
/// 图里高亮的必须是同一根柱
fn dist_caption(range: InsightsRange, peak: usize, peak_n: i64) -> String {
    if peak_n == 0 {
        return "When you talk to your agents".into();
    }
    match range {
        InsightsRange::Hour => format!("Most active around {}", hour_label(peak)),
        InsightsRange::Weekday => format!("Most active on {}", DOW_PLURAL[peak]),
        InsightsRange::Month => format!("Most active in {}", MONTH_FULL[peak]),
    }
}

/// 竖柱分布图(hour 24 根 / weekday 7 根 / month 12 根共用)。峰值柱全饱和
/// primary,其余 55%;零值留 2px muted 底座维持基线连续。宽柱配大缝:
/// 柱数越少 gap 越大,免得 7 根 90px 宽柱糊成一片
fn render_distribution(range: InsightsRange, values: &[i64], peak: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    let gap = match range {
        InsightsRange::Hour => px(4.),
        InsightsRange::Weekday => px(8.),
        InsightsRange::Month => px(6.),
    };
    const CHART_H: f32 = 72.;
    v_flex()
        .gap(px(6.))
        .child(
            h_flex()
                .items_end()
                .gap(gap)
                .h(px(CHART_H))
                .children((0..values.len()).map(|i| {
                    let n = values[i];
                    let (height, bg) = if n == 0 {
                        (px(2.), theme.muted)
                    } else {
                        let frac = (n as f32 / max as f32).max(0.05);
                        (
                            px((frac * CHART_H).max(3.)),
                            if i == peak {
                                theme.primary
                            } else {
                                theme.primary.opacity(0.55)
                            },
                        )
                    };
                    let label: SharedString = match range {
                        InsightsRange::Hour => format!(
                            "{} · {} – {}",
                            prompts_label(n),
                            hour_label(i),
                            hour_label(i + 1)
                        ),
                        InsightsRange::Weekday => {
                            format!("{} · {}", prompts_label(n), DOW_PLURAL[i])
                        }
                        InsightsRange::Month => {
                            format!("{} · {}", prompts_label(n), MONTH_FULL[i])
                        }
                    }
                    .into();
                    div()
                        .id(("dist", i))
                        .flex_1()
                        .h(height)
                        .rounded(RADIUS_CELL)
                        .bg(bg)
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                        })
                })),
        )
        .child(
            // 刻度行:与柱同宽的等分槽。hour 只标 6 小时锚点(文字溢出槽宽
            // 不裁剪),weekday/month 每柱都标
            h_flex()
                .gap(gap)
                .text_size(FONT_LABEL)
                .text_color(theme.muted_foreground)
                .children((0..values.len()).map(|i| {
                    let tick: Option<&'static str> = match range {
                        InsightsRange::Hour => None,
                        InsightsRange::Weekday => Some(DOW_SHORT[i]),
                        InsightsRange::Month => Some(MONTH_SHORT[i]),
                    };
                    div().flex_1().whitespace_nowrap().map(|slot| match tick {
                        // 每柱都有标签时与柱居中;hour 的稀疏锚点靠左
                        Some(t) => slot.flex().justify_center().child(t),
                        None if i % 6 == 0 => slot.child(hour_label(i)),
                        None => slot,
                    })
                })),
        )
        .into_any_element()
}

/// 热力图强度阶梯(primary 不透明度四档)。图例与格子同引本表,
/// 调阶只改这里
const HEAT: [f32; 4] = [0.25, 0.5, 0.75, 1.];

/// GitHub 风活跃热力图:53 周 × 7 天(周一起始),最右列为本周(d.as_of,
/// 与 streak 同一天,渲染层不再读时钟)。格子 9px:网格总宽 26 + 3 +
/// 53×9 + 52×3 = 662,必须收进最小窗口的内容宽 668(940 − 224 侧栏 −
/// 两侧 24 padding)——10px 格的 715 会在最小窗口被裁掉右缘
/// (2026-08-27 Codex review)。daily 升序,二分出窗口后填定长数组——
/// 渲染路径零哈希零日期运算;tooltip 文案 hover 才格式化
fn render_heatmap(d: &InsightsData, cx: &App) -> AnyElement {
    use chrono::Datelike as _;
    let theme = cx.theme();
    let today = d.as_of;
    let this_monday = today - chrono::Days::new(today.weekday().num_days_from_monday() as u64);
    let start = this_monday - chrono::Days::new(52 * 7);
    let today_ix = (today - start).num_days();

    const DAYS: usize = 53 * 7;
    let mut window = [0i64; DAYS];
    let mut heat_max = 1i64;
    let from = d.daily.partition_point(|(day, _)| *day < start);
    for &(day, n) in &d.daily[from..] {
        let ix = (day - start).num_days();
        if (0..DAYS as i64).contains(&ix) {
            window[ix as usize] = n;
            heat_max = heat_max.max(n);
        }
    }
    let heat_color = |n: i64| -> Hsla {
        if n == 0 {
            return theme.muted;
        }
        let quartile = ((n as f32 / heat_max as f32) * 4.).ceil().clamp(1., 4.) as usize;
        theme.primary.opacity(HEAT[quartile - 1])
    };
    const CELL: f32 = 9.;
    const GAP: f32 = 3.;
    const STEP: f32 = CELL + GAP;
    const DOW_W: f32 = 26.;

    // 月份标签:该列周一进入新月份时标注(与前一周比,首列同规则,
    // 相邻标签由此天然隔开 ≥4 列不会叠)
    let mut months = div()
        .relative()
        .w_full()
        .h(px(14.))
        .text_size(FONT_LABEL)
        .text_color(theme.muted_foreground);
    for c in 0..53u64 {
        let monday = start + chrono::Days::new(c * 7);
        if monday.month() != (monday - chrono::Days::new(7)).month() {
            months = months.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(DOW_W + GAP + c as f32 * STEP))
                    .child(MONTH_SHORT[monday.month0() as usize]),
            );
        }
    }

    // 星期标签列:行 r 的格子 y = r×STEP,文字行高 ≈13px,
    // (CELL−13)/2 = −2 光学对行
    let dow_col = div()
        .relative()
        .w(px(DOW_W))
        .h(px(7. * STEP - GAP))
        .flex_shrink_0()
        .text_size(FONT_LABEL)
        .text_color(theme.muted_foreground)
        .children([0usize, 2, 4].map(|r| {
            div()
                .absolute()
                .top(px(r as f32 * STEP - 2.))
                .left_0()
                .child(DOW_SHORT[r])
        }));

    let mut grid = h_flex().gap(px(GAP)).items_start().child(dow_col);
    for c in 0..53usize {
        let mut col = v_flex().gap(px(GAP));
        for r in 0..7usize {
            let ix = c * 7 + r;
            if ix as i64 > today_ix {
                col = col.child(div().size(px(CELL)));
                continue;
            }
            let n = window[ix];
            col = col.child(
                div()
                    .id(("hm", ix))
                    .size(px(CELL))
                    .rounded(RADIUS_CELL)
                    .bg(heat_color(n))
                    // 只捕获 Copy 的 (start, ix, n),hover 到的那格才格式化
                    .tooltip(move |window, cx| {
                        let day = start + chrono::Days::new(ix as u64);
                        let label = format!("{} · {}", prompts_label(n), day.format("%b %-d, %Y"));
                        gpui_component::tooltip::Tooltip::new(SharedString::from(label))
                            .build(window, cx)
                    }),
            );
        }
        grid = grid.child(col);
    }

    // 底注:streak/最忙一天(左) + Less…More 图例(右)
    let mut notes: Vec<String> = Vec::new();
    if d.current_streak > 0 {
        notes.push(format!("{}-day streak", d.current_streak));
    }
    if d.longest_streak > 0 {
        notes.push(format!("Longest {} days", d.longest_streak));
    }
    if let Some((day, n)) = d.busiest_day() {
        notes.push(format!(
            "Busiest {} ({})",
            day.format("%b %-d"),
            prompts_label(n)
        ));
    }
    let legend = h_flex()
        .justify_between()
        .items_center()
        .text_size(FONT_LABEL)
        .text_color(theme.muted_foreground)
        .child(div().min_w_0().truncate().child(notes.join(" · ")))
        .child(
            h_flex()
                .gap(px(GAP))
                .items_center()
                .flex_shrink_0()
                .child("Less")
                .children(std::iter::once(0.).chain(HEAT).map(|a: f32| {
                    div().size(px(CELL)).rounded(RADIUS_CELL).bg(if a == 0. {
                        theme.muted
                    } else {
                        theme.primary.opacity(a)
                    })
                }))
                .child("More"),
        );

    v_flex()
        .gap(px(6.))
        .child(months)
        .child(grid)
        .child(div().pt(SPACE_XS).child(legend))
        .into_any_element()
}

/// 阅读材质空态卡(360px `popover` 圆角面):详情空态与 Insights 空态共用,
/// 形制齐步走
fn empty_state_card(
    icon_path: &'static str,
    circle: Pixels,
    icon_size: Pixels,
    title: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    div()
        .w(px(360.))
        .px(SPACE_XXL)
        .py(SPACE_XXL)
        .rounded(theme.radius_lg)
        .bg(theme.popover)
        .child(empty_state(
            icon_path, circle, icon_size, title, caption, cx,
        ))
}

/// 空态占位(⌘K 初始 / 列表空 / 详情未选中共用):muted 圆底图标 + 标题 + 说明。
/// 圆径/图标径按场景传入,字阶与间距固定,保证三处视觉一致。
fn empty_state(
    icon_path: &'static str,
    circle: Pixels,
    icon_size: Pixels,
    title: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .items_center()
        .gap(SPACE_MD)
        .text_color(theme.muted_foreground)
        .child(
            div()
                .size(circle)
                .rounded_full()
                .bg(theme.muted)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(icon_path)
                        .with_size(icon_size)
                        .text_color(theme.muted_foreground),
                ),
        )
        .child(
            div()
                .text_size(FONT_BODY)
                .font_medium()
                .text_color(theme.foreground)
                .child(title.into()),
        )
        .child(div().text_size(FONT_CAPTION).child(caption.into()))
}

/// markdown 里的一个块:ATX 标题单独切出来,其余内容整段留给 TextView。
///
/// 组件的标题渲染只有 `pb`、没有 `pt`(`Node::Heading`),标题上方的间距
/// 等同于普通段距,层级读不出来;`TextViewStyle` 也无对应钩子。切出来后
/// 上间距由外层 div 给。
///
/// 代价:一条消息拆成多个 TextView,跨块的文字选择会断在块边界。
struct MdBlock {
    /// Some(级别) = ATX 标题;None = 普通内容块
    heading: Option<u8>,
    text: String,
}

/// `#` 到 `######` 后面**跟空格**才是 ATX 标题;`#hashtag` 不是
fn atx_level(line: &str) -> Option<u8> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    match line.as_bytes().get(hashes) {
        Some(b' ') => Some(hashes as u8),
        _ => None,
    }
}

fn split_markdown_blocks(src: &str) -> Vec<MdBlock> {
    let mut out: Vec<MdBlock> = Vec::new();
    let mut buf = String::new();
    // 围栏代码块里的 `# ` 是注释不是标题,必须跟踪开合状态
    let mut fence: Option<String> = None;

    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            }
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(trimmed[..3].to_string());
        }

        match fence.is_none().then(|| atx_level(trimmed)).flatten() {
            Some(level) => {
                if !buf.trim().is_empty() {
                    out.push(MdBlock {
                        heading: None,
                        text: std::mem::take(&mut buf),
                    });
                }
                buf.clear();
                out.push(MdBlock {
                    heading: Some(level),
                    text: line.to_string(),
                });
            }
            None => {
                buf.push_str(line);
                buf.push('\n');
            }
        }
    }
    if !buf.trim().is_empty() {
        out.push(MdBlock {
            heading: None,
            text: buf,
        });
    }
    out
}

/// 标题与上文之间的间距,按层级递减。见 `MdBlock`。
fn heading_top_gap(level: u8) -> Pixels {
    match level {
        1 => px(30.),
        2 => px(25.),
        3 => px(20.),
        _ => px(16.),
    }
}

/// 一条助手消息的正文:按标题切块后逐块渲染,标题块由外层 div 控制上下间距。
fn markdown_message(
    seq: i64,
    text: &str,
    base: Pixels,
    dark: bool,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let blocks = split_markdown_blocks(text);
    let mut col = v_flex().w_full().min_w_0();
    for (ix, block) in blocks.into_iter().enumerate() {
        let id: SharedString = format!("dmsg-{seq}-{ix}").into();
        let body = markdown_body(id, block.text, base, dark, window, cx);
        col = match block.heading {
            // 消息以标题开头时不留上间距,否则会看起来与上一条消息断开
            Some(level) => col.child(
                div()
                    .when(ix > 0, |el| el.pt(heading_top_gap(level)))
                    .pb(px(3.))
                    .child(body),
            ),
            None => col.child(body),
        };
    }
    col
}

/// 对话正文的 markdown 视图,用户气泡与助手平铺共用。组件已内置 table /
/// blockquote / divider / 标题分级 / tree-sitter 高亮,这里只做样式覆写。
fn markdown_body(
    id: SharedString,
    text: String,
    base: Pixels,
    dark: bool,
    window: &mut Window,
    cx: &mut App,
) -> TextView {
    let code_bg = crate::theme::panel_bg(dark);
    let code_border = crate::theme::panel_border(dark);
    // id 必须带深浅模式。TextView 只在首次 request_layout 时同步解析,
    // 之后 style 变化走异步通道(200ms debounce + 后台重解析),而语法高亮
    // 的颜色是解析阶段固化进 `CodeBlock.styles` 的——不换 id 就要慢一拍
    // 才变色。id 变则被视作新元素,走首次的同步解析路径。
    let id: SharedString = format!("{id}-{}", if dark { "d" } else { "l" }).into();
    TextView::markdown(id, text, window, cx)
        .style(
            TextViewStyle {
                heading_base_font_size: base,
                paragraph_gap: gpui::rems(PROSE_PARAGRAPH_GAP),
                is_dark: dark,
                // 必须显式给:默认值恒为 light;且 `TextViewStyle` 的
                // PartialEq 只比较 paragraph_gap / heading_base_font_size /
                // highlight_theme,不换它则深浅切换后 style 被判定相等、
                // 组件不重建,代码块会留着上一个主题的配色
                highlight_theme: if dark {
                    HighlightTheme::default_dark().clone()
                } else {
                    HighlightTheme::default_light().clone()
                },
                ..Default::default()
            }
            // 对话里的标题不该有网页 h1 的体量(组件默认是正文的近两倍),
            // 但差得太小又会混进正文
            .heading_font_size(|level, base| match level {
                1 => base * 1.45,
                2 => base * 1.28,
                3 => base * 1.14,
                4 => base * 1.05,
                _ => base,
            })
            .code_block(
                StyleRefinement::default()
                    .bg(code_bg)
                    .border_1()
                    .border_color(code_border)
                    .rounded(RADIUS_PANEL)
                    .px(px(15.))
                    .py(px(13.)),
            ),
        )
        // 组件把 actions 绝对定位在块内右上角,做不了横跨顶部的工具栏,
        // 所以语言名与复制合成一个胶囊
        .code_block_actions(|block, _window, cx| {
            let theme = cx.theme();
            let code = block.code();
            h_flex()
                .gap(SPACE_SM)
                .items_center()
                .px(px(6.))
                .text_size(FONT_LABEL)
                .text_color(theme.muted_foreground)
                .when_some(block.lang(), |el, lang| el.child(div().child(lang)))
                .child(
                    div()
                        .id("code-copy")
                        .cursor_pointer()
                        .rounded(RADIUS_BADGE)
                        .child(icon("icons/copy.svg").with_size(px(12.)))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Copy code").build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                        }),
                )
        })
        .selectable(true)
}

/// thinking 折叠面板。收起时头行给一句摘要,展开是完整原文。
fn think_panel(
    ix: usize,
    text: &str,
    expanded: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let mut panel = v_flex()
        .w_full()
        .min_w_0()
        .rounded(RADIUS_PANEL)
        .bg(crate::theme::panel_bg(dark))
        .border_1()
        .border_color(crate::theme::panel_border(dark))
        .child(
            h_flex()
                .id(("think", ix))
                .w_full()
                .min_w_0()
                .px(px(13.))
                .py(px(9.))
                .gap(px(7.))
                .items_center()
                .cursor_pointer()
                .text_size(FONT_MSG_THINKING)
                .text_color(theme.muted_foreground)
                .hover(|s| s.text_colored(theme.foreground, FONT_MSG_THINKING))
                .child(
                    icon("icons/chevron-right.svg")
                        .with_size(px(11.))
                        .flex_shrink_0()
                        .when(expanded, |ic| {
                            ic.rotate(gpui::Radians(std::f32::consts::FRAC_PI_2))
                        }),
                )
                .child(div().flex_shrink_0().font_medium().child("Thinking"))
                // 展开后正文就在下面,头行不再重复
                .when(!expanded, |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .italic()
                            .child(one_line(text, 160)),
                    )
                })
                .on_click(on_toggle),
        );
    if expanded {
        panel = panel.child(
            div()
                .w_full()
                .min_w_0()
                // 对齐头行文字起点:13 + 11(图标) + 7(gap)
                .pl(px(31.))
                .pr(px(13.))
                .pb(px(13.))
                .text_size(FONT_MSG_THINKING)
                .line_height(relative(1.75))
                .text_color(theme.muted_foreground)
                .child(text.to_string()),
        );
    }
    panel
}

/// 对话区居中小胶囊(System 消息 / Context compacted)
fn centered_pill(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div().w_full().flex().justify_center().child(
        div()
            .px(px(10.))
            .py(px(3.))
            .rounded_full()
            .bg(theme.muted)
            .text_size(FONT_LABEL)
            .text_color(theme.muted_foreground)
            .max_w(px(520.))
            .truncate()
            .child(text.into()),
    )
}

/// 工具输入输出在折叠体里的展示上限。不用内嵌滚动:嵌套滚动区在虚拟
/// 列表行里会抢滚轮。
const TOOL_OUTPUT_LIMIT: usize = 600;

/// 工具调用折叠卡。头行 = chevron + 名称 + 参数摘要 + 结果徽标,
/// 展开后逐条给出输入与输出(成功的调用也给)。
fn tool_cluster(
    ix: usize,
    calls: &[ToolCallView],
    arg_cells: usize,
    expanded: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let panel_border = crate::theme::panel_border(dark);
    let failed = calls.iter().filter(|c| c.is_error).count();
    let mono = theme.mono_font_family.clone();

    // 单条给"名称 + 参数";多条给数量与名字序列,参数留到展开后逐条看
    let (head_name, head_arg) = if let [only] = calls {
        (
            only.name.clone(),
            Some(clip_display(&only.input_preview, arg_cells)),
        )
    } else {
        let names = calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        (
            format!("{} tool calls", calls.len()),
            Some(clip_display(&names, arg_cells)),
        )
    };

    let mut cluster = v_flex()
        .w_full()
        .min_w_0()
        .rounded(RADIUS_PANEL)
        .bg(crate::theme::panel_bg(dark))
        .border_1()
        .border_color(panel_border)
        .child(
            h_flex()
                .id(("tool-cluster", ix))
                .w_full()
                .min_w_0()
                .px(px(13.))
                .py(px(9.))
                .gap(px(7.))
                .items_center()
                .cursor_pointer()
                .text_size(FONT_MSG_THINKING)
                .text_color(theme.muted_foreground)
                .hover(|s| s.text_colored(theme.foreground, FONT_MSG_THINKING))
                .child(
                    icon("icons/chevron-right.svg")
                        .with_size(px(11.))
                        .flex_shrink_0()
                        .when(expanded, |ic| {
                            ic.rotate(gpui::Radians(std::f32::consts::FRAC_PI_2))
                        }),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_medium()
                        .text_color(theme.foreground)
                        .child(head_name),
                )
                .when_some(head_arg, |el, arg| {
                    el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .text_size(FONT_MSG_MONO)
                            .child(arg),
                    )
                })
                // 结果徽标常驻,不展开也知道有没有失败
                .child(div().flex_1())
                .when(failed > 0, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .px(px(7.))
                            .rounded(RADIUS_BADGE)
                            .text_size(FONT_LABEL)
                            .text_color(theme.danger)
                            .bg(theme.danger.opacity(0.12))
                            .child(format!("{failed} failed")),
                    )
                })
                .on_click(on_toggle),
        );

    if expanded {
        let mut items = v_flex()
            .w_full()
            .min_w_0()
            .border_t_1()
            .border_color(panel_border);
        for (n, tc) in calls.iter().enumerate() {
            let mut item = v_flex()
                .w_full()
                .min_w_0()
                .px(px(13.))
                .py(px(10.))
                .gap(px(6.))
                // 同一簇内多条之间用发丝线分隔,首条不画(卡片自己的顶边已在)
                .when(n > 0, |el| el.border_t_1().border_color(panel_border));
            // 单条时头行已给过名称
            if calls.len() > 1 {
                item = item.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap(px(7.))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(FONT_MSG_THINKING)
                                .font_medium()
                                .text_color(if tc.is_error {
                                    theme.danger
                                } else {
                                    theme.foreground
                                })
                                .child(tc.name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(FONT_MSG_MONO)
                                .font_family(mono.clone())
                                .text_color(theme.muted_foreground)
                                .child(clip_display(&tc.input_preview, arg_cells)),
                        ),
                );
            }
            // 优先给完整 input
            let input = tc
                .input
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| clip_chars(s, TOOL_OUTPUT_LIMIT));
            if let Some(input) = input {
                item = item.child(tool_section("Input", input, false, mono.clone(), cx));
            }
            if let Some(out) = tc
                .output
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                item = item.child(tool_section(
                    "Output",
                    clip_chars(out, TOOL_OUTPUT_LIMIT),
                    tc.is_error,
                    mono.clone(),
                    cx,
                ));
            }
            items = items.child(item);
        }
        cluster = cluster.child(items);
    }
    cluster
}

/// 按码点截断,截断时补省略号
fn clip_chars(s: &str, limit: usize) -> String {
    let mut out: String = s.chars().take(limit).collect();
    if s.chars().nth(limit).is_some() {
        out.push('…');
    }
    out
}

/// 工具卡展开体里的一段(Input / Output)。失败的输出走 danger 色。
fn tool_section(
    label: &'static str,
    body: String,
    is_error: bool,
    mono: SharedString,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(4.))
        .child(
            div()
                .text_size(FONT_LABEL)
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .px(px(10.))
                .py(px(7.))
                .rounded(RADIUS_KBD)
                .bg(crate::theme::inline_code_bg(theme.mode.is_dark()))
                .text_size(FONT_MSG_MONO)
                .line_height(relative(1.65))
                .font_family(mono)
                .text_color(if is_error {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(body),
        )
}

/// 小胶囊 badge(项目名/model/source 共用):4px 圆角,内部截断。
/// 项目名用 muted 灰;model/source 用主题色 tint(淡底+同色文字)。
/// 该数据根是否派生自某条自定义 location(with_custom_root 契约保证派生根
/// 全在其落库目录之下);返回落库路径。面板行标记与表单重叠排除共用同一判据
fn custom_owner<'a>(
    customs: &'a [(AgentId, SharedString)],
    agent: AgentId,
    root: &str,
) -> Option<&'a SharedString> {
    customs
        .iter()
        .find(|(a, p)| *a == agent && path_owns(p.as_ref(), root))
        .map(|(_, p)| p)
}

/// 元信息带里的分隔点。DESIGN.md 规定分隔符是前后带空格的 ` · `;这里
/// 走 flex gap 排版,所以只画点本身,前后空隙由 gap 给
/// jsonl 的 `media_type` → gpui 能渲染的格式。认不出来的(HEIC、AVIF 等)
/// 返回 None,由调用方渲染成"无法解码"占位块
/// 把图片落到下载目录。这张图在磁盘上只以 base64 存在于 jsonl 里,
/// 没有这个入口用户就没有任何办法把它取出来。
fn save_image_to_downloads(image: &gpui::Image) -> Option<std::path::PathBuf> {
    let ext = match image.format {
        gpui::ImageFormat::Png => "png",
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Svg => "svg",
        gpui::ImageFormat::Bmp => "bmp",
        gpui::ImageFormat::Tiff => "tiff",
    };
    // 文件名取内容哈希:同一张图重复导出不会堆出一串 (1)(2)
    let path = dirs::download_dir()?.join(format!("wake-image-{:016x}.{ext}", image.id()));
    std::fs::write(&path, &image.bytes).ok()?;
    Some(path)
}

/// 只读图片头拿原始宽高,不解整张
/// 放大预览里的落地投影。gpui-component 的 `box_shadow` 助手在私有模块里,
/// 直接构造 `BoxShadow`——只有纵向偏移,与实现无关的两个字段固定为 0
/// 放大预览里图片的显示尺寸:等比缩放到可用区域内,**不放大**——
/// 小图放大只会糊,原尺寸看得更清楚。可用区域 = 视口减去四周留白、
/// 底部胶囊(40)与它上方的间距(20)
fn zoom_fit(dims: Option<(u32, u32)>, viewport: Size<Pixels>) -> Size<Pixels> {
    let avail_w = (f32::from(viewport.width) - 56. * 2.).max(120.);
    let avail_h = (f32::from(viewport.height) - 52. * 2. - 40. - 20.).max(120.);
    let Some((w, h)) = dims.filter(|(w, h)| *w > 0 && *h > 0) else {
        return gpui::size(px(avail_w.min(720.)), px(avail_h.min(480.)));
    };
    let (w, h) = (w as f32, h as f32);
    let scale = (avail_w / w).min(avail_h / h).min(1.0);
    gpui::size(px(w * scale), px(h * scale))
}

fn zoom_shadow(y: Pixels, blur: Pixels, alpha: f32) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: gpui::black().opacity(alpha),
        offset: gpui::point(px(0.), y),
        blur_radius: blur,
        spread_radius: px(0.),
    }
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn image_format_of(media_type: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat as F;
    Some(match media_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => F::Png,
        "image/jpeg" | "image/jpg" => F::Jpeg,
        "image/webp" => F::Webp,
        "image/gif" => F::Gif,
        "image/svg+xml" => F::Svg,
        "image/bmp" => F::Bmp,
        "image/tiff" | "image/tif" => F::Tiff,
        _ => return None,
    })
}

/// 用户气泡内的图片网格。统一 `IMAGE_THUMB` 见方,靠 `landscape` 决定钉高
/// 还是钉宽、另一边溢出后由外层 `overflow_hidden` 居中裁掉——等价于
/// CSS 的 `object-fit: cover`,gpui 的 `img()` 没有这个属性,只能这么做。
fn image_strip(
    msg_ix: usize,
    slots: &[ImageSlot],
    theme_border: Hsla,
    panel: Hsla,
    muted: Hsla,
    workbench: Entity<Workbench>,
) -> Div {
    let mut row = h_flex().flex_wrap().gap(SPACE_SM);
    for (i, slot) in slots.iter().enumerate() {
        row = row.child(match slot {
            ImageSlot::Ready { image, dims } => {
                let wb = workbench.clone();
                let fig = gpui::img(image.clone());
                // 钉短边、长边溢出:横图钉高、竖图钉宽,两种都能填满方格。
                // 尺寸读不出来时按横图处理(截屏基本都是宽大于高)
                let landscape = dims.map(|(w, h)| w >= h).unwrap_or(true);
                let fig = if landscape {
                    fig.h(IMAGE_THUMB)
                } else {
                    fig.w(IMAGE_THUMB)
                };
                div()
                    .id(("shot", msg_ix * IMAGES_PER_MSG + i))
                    .size(IMAGE_THUMB)
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(RADIUS_IMAGE)
                    .border_1()
                    .border_color(theme_border)
                    .bg(panel)
                    .cursor_pointer()
                    .child(fig)
                    .on_click(move |_, _window, cx| {
                        wb.update(cx, |this, cx| {
                            if let Some(detail) = &mut this.detail {
                                detail.zoom = Some((msg_ix, i));
                                cx.notify();
                            }
                        });
                    })
                    .into_any_element()
            }
            ImageSlot::Unsupported(mt) => v_flex()
                .size(IMAGE_THUMB)
                .items_center()
                .justify_center()
                .gap(SPACE_XS)
                .p(SPACE_SM)
                .rounded(RADIUS_IMAGE)
                .border_1()
                .border_color(theme_border)
                .bg(panel)
                .text_size(FONT_LABEL)
                .text_color(muted)
                .child("无法解码")
                .child(div().truncate().child(mt.clone()))
                .into_any_element(),
        });
    }
    row
}

fn meta_sep(color: Hsla) -> Div {
    div()
        .flex_shrink_0()
        .text_color(color.opacity(0.55))
        .child("·")
}

fn badge(name: impl Into<SharedString>, bg: Hsla, fg: Hsla) -> impl IntoElement {
    div()
        .min_w_0()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_BADGE)
        .bg(bg)
        .text_color(fg)
        .font_medium()
        .child(div().truncate().child(name.into()))
}

/// outline 变体(model/source 用):透明底,边框与文字同色
fn outline_badge(name: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .min_w_0()
        .px(px(6.))
        .py(px(1.))
        .rounded(RADIUS_BADGE)
        .border_1()
        .border_color(color)
        .text_color(color)
        .font_medium()
        .child(div().truncate().child(name.into()))
}

/// 侧栏底部工具条的图标按钮。透明底、hover 才出色——底部是次要操作区,
/// 不与导航行的选中态抢注意力;图标-only 元素改 text_color 不丢字号。
/// enabled=false(刷新进行中)只留静态内容,连 tooltip 与点击一起摘掉
fn sidebar_tool_btn(
    id: &'static str,
    tooltip: &'static str,
    enabled: bool,
    content: AnyElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &Context<Workbench>,
) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .size(ROW_HEIGHT)
        .flex_shrink_0()
        .rounded(theme.radius)
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.muted_foreground)
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(|s| s.bg(theme.secondary_hover).text_color(theme.foreground))
                .active(|s| s.bg(theme.secondary_active))
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
                })
                .on_click(on_click)
        })
        .child(content)
}

/// Things 风源列表行:图标 + 文字 + 计数,6px 圆角选中胶囊
#[allow(clippy::too_many_arguments)]
fn sidebar_row(
    id: impl Into<ElementId>,
    lead: RowLead,
    label: impl Into<SharedString>,
    count: Option<i64>,
    active: bool,
    level: RowLevel,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &Context<Workbench>,
) -> Stateful<Div> {
    let theme = cx.theme();
    let sub = level == RowLevel::Sub;
    div()
        .id(id)
        .h(if sub { ROW_HEIGHT_SUB } else { ROW_HEIGHT })
        .flex_shrink_0()
        // 分组项整行右移一档表达从属:行首因此落在轴右侧 SUB_INDENT 处,
        // 压轴的是主导航与组头,不是这里
        .pl(if sub {
            LEAD_INSET + SUB_INDENT
        } else {
            LEAD_INSET
        })
        .pr(SIDEBAR_EDGE)
        .rounded(theme.radius)
        .cursor_pointer()
        .flex()
        .items_center()
        .when(active, |s| {
            s.bg(theme.sidebar_accent)
                .text_color(theme.sidebar_accent_foreground)
        })
        .when(!active, |s| {
            s.text_color(theme.sidebar_foreground)
                .hover(|s| s.bg(theme.sidebar_accent.opacity(0.55)))
                .active(|s| s.bg(theme.sidebar_accent))
        })
        .on_click(on_click)
        .child(
            h_flex()
                .w_full()
                .gap(SPACE_SM)
                .child(
                    // 定宽槽位保证文字起点统一;内部居中,使小图标的中心也落在
                    // LEAD_AXIS 上(左对齐会让 14/15px 图标的中心偏离轴 1.5~2pt)
                    div()
                        .w(LEAD_BOX)
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(match lead {
                            // 线条图标比实心品牌图视觉轻,给它小一档才平衡
                            RowLead::Icon(ic) => ic
                                .with_size(if sub { px(14.) } else { px(15.) })
                                .text_color(if active {
                                    theme.sidebar_accent_foreground
                                } else {
                                    theme.muted_foreground
                                })
                                .into_any_element(),
                            // 品牌图不着色:img 走 AssetSource 取内嵌 PNG,原色渲染
                            // (侧栏单色化试过,用户否决——保持彩色)
                            RowLead::Brand(path) => img(path).size(LEAD_BOX).into_any_element(),
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(if sub { FONT_CAPTION } else { FONT_BODY })
                        .truncate()
                        .child(label.into()),
                )
                .when_some(count, |this, n| {
                    // 与 Session locations 面板同款胶囊。底色随行态切换而不是
                    // 固定 muted:常态行底是 sidebar,用 accent 衬;选中行底本身
                    // 就是 accent,退回 sidebar 材质反衬。固定 muted 会在浅色
                    // 常态(#E8E8E5 vs #EDEDEA)和深色选中(#323230 vs #343432)
                    // 两处糊进背景里
                    let bg = if active {
                        theme.sidebar
                    } else {
                        theme.sidebar_accent
                    };
                    this.child(div().flex_shrink_0().text_size(FONT_LABEL).child(badge(
                        n.to_string(),
                        bg,
                        theme.muted_foreground,
                    )))
                }),
        )
}

/// 详情工具栏图标按钮。选中态 = 填充版图标 + 语义色(macOS 惯例),
/// 按钮本身不落底色(.selected 的 hover 底被否)。
fn tool_btn(
    id: &'static str,
    icon_path: &'static str,
    filled_icon_path: &'static str,
    active_color: Hsla,
    tooltip: &'static str,
    highlighted: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let ic = if highlighted {
        icon(filled_icon_path)
            .with_size(px(16.))
            .text_color(active_color)
    } else {
        icon(icon_path).with_size(px(16.))
    };
    Button::new(id)
        .ghost()
        .rounded(RADIUS_BUTTON)
        .icon(ic)
        .tooltip(tooltip)
        .on_click(on_click)
}

impl Focusable for Workbench {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id("workbench")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_search))
            .on_action(cx.listener(|this, _: &RefreshSessions, window, cx| {
                this.refresh_sessions(window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _window, cx| this.open_settings(cx)))
            .on_action(cx.listener(|this, _: &OpenUpdates, _window, cx| this.open_updates(cx)))
            .on_action(cx.listener(|this, _: &OpenAbout, _window, cx| this.open_about(cx)))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .size_full()
                    .child(self.render_sidebar(window, cx))
                    // Insights 是整页目的地:替换中栏+右栏,侧栏导航保持在场
                    .map(|this| {
                        if self.insights_open {
                            this.child(self.render_insights(cx))
                        } else {
                            this.child(self.render_session_list(cx))
                                .child(self.render_detail(window, cx))
                        }
                    }),
            )
            .child(self.render_image_zoom(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

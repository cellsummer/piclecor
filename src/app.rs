use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode, Disableable, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::config::{Appearance, Config, Destination};
use crate::library::{self, Kind};
use crate::transfer::{self, Outcome, Reach};

const SIDEBAR_W: f32 = 320.;
const GRID_PAD: f32 = 12.;
const CELL_W: f32 = 168.;
const CELL_H: f32 = 156.;
const THUMB_H: f32 = 118.;
const CHECK_SIZE: f32 = 22.;
const SELECTED_TINT: f32 = 0.18;
const CELL_GROUP: &str = "cell";

/// Thumbnails render in parallel batches; the UI refreshes after each.
const THUMB_BATCH: usize = 8;
const TICK: Duration = Duration::from_millis(250);

const GRID_CONTEXT: &str = "Piclecor";
const PREVIEW_CONTEXT: &str = "Preview";

actions!(piclecor, [SelectAll, PrevPhoto, NextPhoto, ToggleSelect, ClosePreview]);

/// Preview keys live in their own context so sidebar inputs keep space and arrows.
pub fn bind_keys(cx: &mut App) {
    let p = Some(PREVIEW_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-a", SelectAll, Some(GRID_CONTEXT)),
        KeyBinding::new("left", PrevPhoto, p),
        KeyBinding::new("right", NextPhoto, p),
        KeyBinding::new("space", ToggleSelect, p),
        KeyBinding::new("enter", ToggleSelect, p),
        KeyBinding::new("s", ToggleSelect, p),
        KeyBinding::new("escape", ClosePreview, p),
    ]);
}

struct Photo {
    path: PathBuf,
    thumb: Option<PathBuf>,
}

struct Job {
    label: String,
    progress: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
enum Tone {
    Good,
    Bad,
    Neutral,
}

#[derive(Clone, Copy)]
enum Pick {
    Toggle,
    Range,
}

pub struct Piclecor {
    focus: FocusHandle,
    preview_focus: FocusHandle,
    config: Config,
    folder: Option<PathBuf>,
    photos: Vec<Photo>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    preview: Option<usize>,
    preview_img: Option<PathBuf>,
    volumes: Vec<PathBuf>,
    name_input: Entity<InputState>,
    target_input: Entity<InputState>,
    job: Option<Job>,
    /// Keyed by target; absent means a check is running.
    reach: HashMap<String, Reach>,
    adding: bool,
    form_msg: Option<(SharedString, Tone)>,
    status: SharedString,
    scroll: UniformListScrollHandle,
    // Dropping a task cancels it, so replacing these stops stale work.
    scan_task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    job_task: Option<Task<()>>,
    tick_task: Option<Task<()>>,
}

impl Piclecor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = Config::load();
        let last = config.last_folder.clone();
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let target_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("/Volumes/Disk/Photos or user@host:/photos")
        });

        let mut this = Self {
            focus: cx.focus_handle(),
            preview_focus: cx.focus_handle(),
            config,
            folder: None,
            photos: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            preview: None,
            preview_img: None,
            volumes: library::camera_volumes(),
            name_input,
            target_input,
            job: None,
            reach: HashMap::new(),
            adding: false,
            form_msg: None,
            status: "Open a folder or a card.".into(),
            scroll: UniformListScrollHandle::new(),
            scan_task: None,
            preview_task: None,
            job_task: None,
            tick_task: None,
        };

        if let Some(dir) = last.filter(|d| d.is_dir()) {
            this.open(dir, cx);
        }
        let targets: Vec<String> = this.config.destinations.iter().map(|d| d.target.clone()).collect();
        for t in targets {
            this.check_target(t, cx);
        }
        this.apply_appearance(window, cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            if this.config.appearance == Appearance::System {
                Theme::sync_system_appearance(Some(window), cx);
            }
        })
        .detach();

        window.focus(&this.focus, cx);
        this
    }

    fn apply_appearance(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.config.appearance {
            Appearance::System => Theme::sync_system_appearance(Some(window), cx),
            Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
        window.refresh();
    }

    fn cycle_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.appearance = self.config.appearance.next();
        self.persist();
        self.apply_appearance(window, cx);
        cx.notify();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| this.open(dir, cx)).ok();
        })
        .detach();
    }

    fn open(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.photos.clear();
        self.selected.clear();
        self.anchor = None;
        self.close_preview();
        self.folder = Some(dir.clone());
        self.status = format!("Scanning {}…", dir.display()).into();

        self.config.last_folder = Some(dir.clone());
        self.persist();

        self.scan_task = Some(cx.spawn(async move |this, cx| {
            let scan_dir = dir.clone();
            let paths = cx
                .background_executor()
                .spawn(async move { library::scan(&scan_dir) })
                .await;

            let ok = this.update(cx, |this, cx| {
                this.photos = paths
                    .iter()
                    .map(|p| Photo { path: p.clone(), thumb: None })
                    .collect();
                this.status = format!("{} photos", this.photos.len()).into();
                cx.notify();
            });
            if ok.is_err() {
                return;
            }

            for (n, batch) in paths.chunks(THUMB_BATCH).enumerate() {
                let batch = batch.to_vec();
                let thumbs = cx
                    .background_executor()
                    .spawn(async move { render_all(&batch, Kind::Thumb) })
                    .await;

                let start = n * THUMB_BATCH;
                let ok = this.update(cx, |this, cx| {
                    for (i, thumb) in thumbs.into_iter().enumerate() {
                        if let Some(photo) = this.photos.get_mut(start + i) {
                            photo.thumb = thumb;
                        }
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn click(&mut self, ix: usize, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if ev.click_count() == 2 {
            self.show_preview(ix, window, cx);
            return;
        }
        let pick = if ev.modifiers().shift { Pick::Range } else { Pick::Toggle };
        self.pick(ix, pick);
        cx.notify();
    }

    fn pick(&mut self, ix: usize, pick: Pick) {
        match (pick, self.anchor) {
            (Pick::Range, Some(a)) => {
                let (lo, hi) = if a <= ix { (a, ix) } else { (ix, a) };
                self.selected.extend(lo..=hi);
            }
            _ => {
                if !self.selected.remove(&ix) {
                    self.selected.insert(ix);
                }
            }
        }
        self.anchor = Some(ix);
    }

    fn show_preview(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(photo) = self.photos.get(ix) else { return };
        let src = photo.path.clone();
        self.preview = Some(ix);
        self.anchor = Some(ix);
        window.focus(&self.preview_focus, cx);
        self.preview_img = None;

        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move { library::render(&src, Kind::Preview) })
                .await;
            this.update(cx, |this, cx| {
                this.preview_img = out;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn close_preview(&mut self) {
        self.preview = None;
        self.preview_img = None;
        self.preview_task = None;
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.preview else { return };
        let Some(next) = i.checked_add_signed(delta) else { return };
        if next < self.photos.len() {
            self.show_preview(next, window, cx);
        }
    }

    fn toggle_current(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.preview {
            self.pick(i, Pick::Toggle);
            cx.notify();
        }
    }

    fn leave_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_preview();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn check_target(&mut self, target: String, cx: &mut Context<Self>) {
        self.reach.remove(&target);
        cx.spawn(async move |this, cx| {
            let t = target.clone();
            let reach = cx.background_executor().spawn(async move { transfer::check(&t) }).await;
            this.update(cx, |this, cx| {
                this.reach.insert(target, reach);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Only adds a destination whose folder exists, so typos surface now and not mid-copy.
    fn add_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_string();
        let target = transfer::unquote(self.target_input.read(cx).value().trim());
        if target.is_empty() {
            self.form_msg = Some(("Enter a folder path.".into(), Tone::Bad));
            cx.notify();
            return;
        }
        let name = if name.is_empty() { target.clone() } else { name };

        self.adding = true;
        self.form_msg = Some((format!("Checking {target}…").into(), Tone::Neutral));
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let t = target.clone();
            let reach = cx.background_executor().spawn(async move { transfer::check(&t) }).await;
            this.update_in(cx, |this, window, cx| {
                this.adding = false;
                this.form_msg = Some(match &reach {
                    Reach::Ready => (format!("Added {name}. Folder found.").into(), Tone::Good),
                    Reach::Missing => (format!("Folder not found: {target}").into(), Tone::Bad),
                    Reach::Unreachable(e) => (format!("Cannot reach host: {e}").into(), Tone::Bad),
                });
                if matches!(reach, Reach::Ready) {
                    this.config.destinations.push(Destination { name, target: target.clone() });
                    this.persist();
                    this.reach.insert(target, reach);
                    this.name_input.update(cx, |s, cx| s.set_value("", window, cx));
                    this.target_input.update(cx, |s, cx| s.set_value("", window, cx));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_destination(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.config.destinations.len() {
            self.config.destinations.remove(ix);
            self.persist();
            cx.notify();
        }
    }

    fn persist(&mut self) {
        if let Err(e) = self.config.save() {
            self.status = format!("Config not saved: {e}").into();
        }
    }

    fn copy_selected(&mut self, dest: usize, cx: &mut Context<Self>) {
        let Some(d) = self.config.destinations.get(dest).cloned() else { return };
        let files: Vec<PathBuf> = self
            .selected
            .iter()
            .filter_map(|&i| self.photos.get(i).map(|p| p.path.clone()))
            .collect();
        if files.is_empty() {
            return;
        }

        let label = format!("Copying {} to {}", files.len(), d.name);
        self.start_job(label, d.target.clone(), cx, move |progress| {
            transfer::copy_files(&files, &d.target, progress)
        });
    }

    fn sync_folder(&mut self, dest: usize, cx: &mut Context<Self>) {
        let Some(d) = self.config.destinations.get(dest).cloned() else { return };
        let Some(src) = self.folder.clone() else { return };

        let label = format!("Backing up {} to {}", short(&src), d.name);
        self.start_job(label, d.target.clone(), cx, move |progress| {
            transfer::sync_tree(&src, &d.target, progress)
        });
    }

    fn start_job(
        &mut self,
        label: String,
        target: String,
        cx: &mut Context<Self>,
        work: impl FnOnce(Arc<AtomicUsize>) -> Outcome + Send + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        let progress = Arc::new(AtomicUsize::new(0));
        self.job = Some(Job { label: label.clone(), progress: progress.clone() });

        self.tick_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.job.is_some()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        }));

        self.job_task = Some(cx.spawn(async move |this, cx| {
            let out = cx.background_executor().spawn(async move { work(progress) }).await;
            this.update(cx, |this, cx| {
                this.job = None;
                this.status = match out.error {
                    None => format!("{label}: done, {} new files.", out.sent),
                    Some(e) => format!("{label}: {} new files, then {e}", out.sent),
                }
                .into();
                this.check_target(target, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn cols(window: &Window) -> usize {
        let w = f32::from(window.viewport_size().width) - SIDEBAR_W - GRID_PAD * 2.;
        ((w / CELL_W) as usize).max(1)
    }

    fn render_rows(&mut self, rows: Range<usize>, cols: usize, cx: &mut Context<Self>) -> Vec<Div> {
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let start = r * cols;
            let end = (start + cols).min(self.photos.len());
            let mut row = h_flex().px(px(GRID_PAD));
            for ix in start..end {
                row = row.child(self.render_cell(ix, cx));
            }
            out.push(row);
        }
        out
    }

    fn render_cell(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let photo = &self.photos[ix];
        let selected = self.selected.contains(&ix);
        let name = photo
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let thumb = match &photo.thumb {
            Some(p) => img(p.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element(),
            None => div().size_full().bg(theme.muted).into_any_element(),
        };

        // Hollow ring on hover hints that a click selects; filled check once selected.
        let check = div()
            .absolute()
            .top_1()
            .right_1()
            .size(px(CHECK_SIZE))
            .rounded_full()
            .border_2()
            .border_color(white())
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_semibold()
            .map(|d| match selected {
                true => d.bg(theme.primary).text_color(theme.primary_foreground).child("✓"),
                false => d.bg(black().opacity(0.3)).invisible().group_hover(CELL_GROUP, |s| s.visible()),
            });

        v_flex()
            .id(("cell", ix))
            .group(CELL_GROUP)
            .w(px(CELL_W))
            .h(px(CELL_H))
            .p_1()
            .gap_1()
            .rounded_lg()
            // Constant width keeps the grid from shifting when selection changes.
            .border_4()
            .border_color(if selected { theme.primary } else { transparent_black() })
            .when(selected, |d| d.bg(theme.primary.opacity(SELECTED_TINT)))
            .when(!selected, |d| d.hover(|d| d.bg(theme.accent)))
            .on_click(cx.listener(move |this, ev, window, cx| this.click(ix, ev, window, cx)))
            .child(div().relative().w_full().h(px(THUMB_H)).child(thumb).child(check))
            .child(
                div()
                    .text_xs()
                    .truncate()
                    .map(|d| match selected {
                        true => d.font_semibold().text_color(theme.primary),
                        false => d.text_color(theme.muted_foreground),
                    })
                    .child(name),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let folder = self
            .folder
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_else(|| "No folder".into());
        let count = format!("{} photos · {} selected", self.photos.len(), self.selected.len());

        h_flex()
            .p(px(GRID_PAD))
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().text_sm().truncate().child(folder))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(count)),
            )
            .child(
                Button::new("theme")
                    .small()
                    .ghost()
                    .label(self.config.appearance.label())
                    .on_click(cx.listener(|this, _, window, cx| this.cycle_appearance(window, cx))),
            )
            .child(
                Button::new("all")
                    .small()
                    .label("Select all")
                    .disabled(self.photos.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = (0..this.photos.len()).collect();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("none")
                    .small()
                    .label("Clear")
                    .disabled(self.selected.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected.clear();
                        this.anchor = None;
                        cx.notify();
                    })),
            )
    }

    fn render_grid(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cols = Self::cols(window);
        let rows = self.photos.len().div_ceil(cols);

        uniform_list(
            "grid",
            rows,
            cx.processor(move |this, range, _, cx| this.render_rows(range, cols, cx)),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .py(px(GRID_PAD))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let busy = self.job.is_some();

        let heading = |s: &'static str| {
            div().text_xs().font_semibold().text_color(theme.muted_foreground).child(s)
        };

        let sources = v_flex()
            .gap_2()
            .child(heading("SOURCE"))
            .child(
                Button::new("open")
                    .small()
                    .label("Open folder…")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
            )
            .children(self.volumes.iter().enumerate().map(|(i, v)| {
                let dir = v.join("DCIM");
                Button::new(("vol", i))
                    .small()
                    .ghost()
                    .label(format!("Card: {}", short(v)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(dir.clone(), cx)))
            }))
            .child(
                Button::new("rescan")
                    .xsmall()
                    .ghost()
                    .label("Find cards")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.volumes = library::camera_volumes();
                        cx.notify();
                    })),
            );

        let n_sel = self.selected.len();
        let dests = self.config.destinations.iter().enumerate().map(|(i, d)| {
            v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .justify_between()
                        .child(div().text_sm().font_semibold().truncate().child(d.name.clone()))
                        .child(
                            Button::new(("rm", i))
                                .xsmall()
                                .ghost()
                                .label("Remove")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_destination(i, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(d.target.clone()),
                )
                .child(self.render_reach(&d.target, cx))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(("copy", i))
                                .small()
                                .primary()
                                .label(format!("Copy {n_sel}"))
                                .disabled(busy || n_sel == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy_selected(i, cx)
                                })),
                        )
                        .child(
                            Button::new(("sync", i))
                                .small()
                                .label("Back up folder")
                                .tooltip("Copy every new file in the open folder. Never deletes.")
                                .disabled(busy || self.folder.is_none())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.sync_folder(i, cx)
                                })),
                        ),
                )
        });

        let add = v_flex()
            .gap_2()
            .child(Input::new(&self.name_input).small())
            .child(Input::new(&self.target_input).small())
            .child(
                Button::new("add")
                    .small()
                    .label("Add destination")
                    .loading(self.adding)
                    .disabled(self.adding)
                    .on_click(cx.listener(|this, _, window, cx| this.add_destination(window, cx))),
            )
            .when_some(self.form_msg.clone(), |d, (msg, tone)| {
                d.child(div().text_xs().text_color(tone_color(tone, cx)).child(msg))
            });

        let status = match &self.job {
            Some(j) => format!("{}… {} sent", j.label, j.progress.load(Ordering::Relaxed)),
            None => self.status.to_string(),
        };

        v_flex()
            .w(px(SIDEBAR_W))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                v_flex()
                    .id("sidebar")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_4()
                    .child(sources)
                    .child(v_flex().gap_2().child(heading("DESTINATIONS")).children(dests))
                    .child(v_flex().gap_2().child(heading("NEW DESTINATION")).child(add)),
            )
            .child(
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(status),
            )
    }

    fn render_reach(&self, target: &str, cx: &App) -> impl IntoElement {
        let (msg, tone): (SharedString, Tone) = match self.reach.get(target) {
            None => ("Checking…".into(), Tone::Neutral),
            Some(Reach::Ready) => ("✓ Folder found".into(), Tone::Good),
            Some(Reach::Missing) => ("✗ Folder not found".into(), Tone::Bad),
            Some(Reach::Unreachable(e)) => (format!("✗ Cannot reach host: {e}").into(), Tone::Bad),
        };
        div()
            .text_xs()
            .truncate()
            .text_color(tone_color(tone, cx))
            .child(msg)
    }

    fn render_preview(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let photo = &self.photos[ix];
        let selected = self.selected.contains(&ix);
        let src = self.preview_img.clone().or_else(|| photo.thumb.clone());
        let position = format!("{} / {}", ix + 1, self.photos.len());
        let frame = if selected { theme.primary } else { transparent_black() };

        let badge = div()
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .font_semibold()
            .map(|d| match selected {
                true => d.bg(theme.primary).text_color(theme.primary_foreground).child("✓ Selected"),
                false => d.bg(theme.muted).text_color(theme.muted_foreground).child("Not selected"),
            });

        let toggle = Button::new("pv-toggle")
            .small()
            .label(if selected { "Deselect" } else { "Select" })
            .when(!selected, |b| b.primary())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_current(cx)));

        let bar = h_flex()
            .p_3()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(badge)
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().text_sm().truncate().child(short(&photo.path)))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(position)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("←/→ browse · Space select · Esc close"),
            )
            .child(
                Button::new("pv-prev")
                    .small()
                    .label("←")
                    .disabled(ix == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
            )
            .child(
                Button::new("pv-next")
                    .small()
                    .label("→")
                    .disabled(ix + 1 >= self.photos.len())
                    .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
            )
            .child(toggle)
            .child(
                Button::new("pv-close")
                    .small()
                    .ghost()
                    .label("Close")
                    .on_click(cx.listener(|this, _, window, cx| this.leave_preview(window, cx))),
            );

        // Photos read best on black in either theme.
        let stage = div().flex_1().p_4().bg(black()).child(
            div()
                .size_full()
                .p_1()
                .rounded_md()
                .border_4()
                .border_color(frame)
                .when_some(src, |d, p| d.child(img(p).size_full().object_fit(ObjectFit::Contain))),
        );

        v_flex()
            .id("preview")
            .key_context(PREVIEW_CONTEXT)
            .track_focus(&self.preview_focus)
            .absolute()
            .inset_0()
            .occlude()
            .on_action(cx.listener(|this, _: &PrevPhoto, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &NextPhoto, window, cx| this.step(1, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSelect, _, cx| this.toggle_current(cx)))
            .on_action(cx.listener(|this, _: &ClosePreview, window, cx| this.leave_preview(window, cx)))
            .child(bar)
            .child(stage)
    }
}

fn render_all(paths: &[PathBuf], kind: Kind) -> Vec<Option<PathBuf>> {
    std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .iter()
            .map(|p| s.spawn(move || library::render(p, kind)))
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    })
}

fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Good => theme.success,
        Tone::Bad => theme.danger,
        Tone::Neutral => theme.muted_foreground,
    }
}

fn short(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl Render for Piclecor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .size_full()
            .relative()
            .bg(theme.background)
            .text_color(theme.foreground)
            .key_context(GRID_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.selected = (0..this.photos.len()).collect();
                cx.notify();
            }))
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .child(self.render_toolbar(cx))
                    .child(self.render_grid(window, cx)),
            )
            .child(self.render_sidebar(cx))
            .when_some(self.preview, |d, ix| d.child(self.render_preview(ix, cx)))
    }
}

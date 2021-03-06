use crate::app::{App, FindDelayedIntersections, ShowEverything};
use crate::common::Warping;
use crate::game::{DrawBaselayer, PopupMsg, State, Transition};
use crate::helpers::ID;
use crate::render::DrawOptions;
use crate::sandbox::{GameplayMode, SandboxMode};
use abstutil::prettyprint_usize;
use geom::{Duration, Polygon, Pt2D, Ring, Time};
use instant::Instant;
use widgetry::{
    hotkey, AreaSlider, Btn, Checkbox, Choice, Color, EventCtx, GeomBatch, GfxCtx, Key, Line,
    Outcome, Panel, PersistentSplit, Text, UpdateType, Widget,
};

// TODO Text entry would be great
pub struct JumpToTime {
    panel: Panel,
    target: Time,
    halt_limit: Duration,
    maybe_mode: Option<GameplayMode>,
}

impl JumpToTime {
    pub fn new(ctx: &mut EventCtx, app: &App, maybe_mode: Option<GameplayMode>) -> JumpToTime {
        let target = app.primary.sim.time();
        let end_of_day = app.primary.sim.get_end_of_day();
        let halt_limit = app.opts.time_warp_halt_limit;
        JumpToTime {
            target,
            halt_limit,
            maybe_mode,
            panel: Panel::new(Widget::col(vec![
                Widget::row(vec![
                    Line("Jump to what time?").small_heading().draw(ctx),
                    Btn::plaintext("X")
                        .build(ctx, "close", hotkey(Key::Escape))
                        .align_right(),
                ]),
                Checkbox::checkbox(
                    ctx,
                    "skip drawing (for faster simulations)",
                    None,
                    app.opts.dont_draw_time_warp,
                )
                .margin_above(30)
                .named("don't draw"),
                Widget::horiz_separator(ctx, 0.25).margin_above(10),
                if app.has_prebaked().is_some() {
                    Widget::draw_batch(
                        ctx,
                        GeomBatch::from(vec![(
                            Color::WHITE.alpha(0.7),
                            area_under_curve(
                                app.prebaked().active_agents(end_of_day),
                                // TODO Auto fill width
                                500.0,
                                50.0,
                            ),
                        )]),
                    )
                } else {
                    Widget::nothing()
                },
                // TODO Auto-fill width?
                AreaSlider::new(
                    ctx,
                    0.25 * ctx.canvas.window_width,
                    target.to_percent(end_of_day).min(1.0),
                )
                .named("time slider")
                .centered_horiz()
                // EZGUI FIXME: margin_below having no effect here, so instead we add a margin_top
                // to the subsequent element
                //.margin_above(16).margin_below(16),
                .margin_above(16),
                build_jump_to_time_btn(target, ctx),
                Widget::horiz_separator(ctx, 0.25).margin_above(16),
                build_jump_to_delay_picker(halt_limit, ctx).margin_above(16),
                build_jump_to_delay_button(halt_limit, ctx),
            ]))
            .build(ctx),
        }
    }
}

impl State for JumpToTime {
    fn event(&mut self, ctx: &mut EventCtx, app: &mut App) -> Transition {
        match self.panel.event(ctx) {
            Outcome::Clicked(x) => match x.as_ref() {
                "close" => {
                    return Transition::Pop;
                }
                "jump to time" => {
                    if self.target < app.primary.sim.time() {
                        if let Some(mode) = self.maybe_mode.take() {
                            return Transition::Multi(vec![
                                Transition::Replace(SandboxMode::new(ctx, app, mode)),
                                Transition::Push(TimeWarpScreen::new(ctx, app, self.target, None)),
                            ]);
                        } else {
                            return Transition::Replace(PopupMsg::new(
                                ctx,
                                "Error",
                                vec!["Sorry, you can't go rewind time from this mode."],
                            ));
                        }
                    }
                    return Transition::Replace(TimeWarpScreen::new(ctx, app, self.target, None));
                }
                "choose delay" => return Transition::Keep,
                "jump to delay" => {
                    let halt_limit = self.panel.persistent_split_value("choose delay");
                    app.opts.time_warp_halt_limit = halt_limit;
                    return Transition::Replace(TimeWarpScreen::new(
                        ctx,
                        app,
                        app.primary.sim.get_end_of_day(),
                        Some(halt_limit),
                    ));
                }
                _ => unreachable!(),
            },
            Outcome::Changed => {
                app.opts.dont_draw_time_warp = self.panel.is_checked("don't draw");
            }
            _ => {}
        }
        let target = app
            .primary
            .sim
            .get_end_of_day()
            .percent_of(self.panel.area_slider("time slider").get_percent())
            .round_seconds(600.0);
        if target != self.target {
            self.target = target;
            self.panel
                .replace(ctx, "jump to time", build_jump_to_time_btn(target, ctx));
        }

        let halt_limit = self.panel.persistent_split_value("choose delay");
        if halt_limit != self.halt_limit {
            self.halt_limit = halt_limit;
            self.panel.replace(
                ctx,
                "jump to delay",
                build_jump_to_delay_button(halt_limit, ctx),
            );
        }

        if self.panel.clicked_outside(ctx) {
            return Transition::Pop;
        }

        Transition::Keep
    }

    fn draw(&self, g: &mut GfxCtx, app: &App) {
        State::grey_out_map(g, app);
        self.panel.draw(g);
    }
}

// Display a nicer screen for jumping forwards in time, allowing cancellation.
pub struct TimeWarpScreen {
    target: Time,
    wall_time_started: Instant,
    sim_time_started: geom::Time,
    halt_upon_delay: Option<Duration>,
    panel: Panel,
}

impl TimeWarpScreen {
    pub fn new(
        ctx: &mut EventCtx,
        app: &mut App,
        target: Time,
        mut halt_upon_delay: Option<Duration>,
    ) -> Box<dyn State> {
        if let Some(halt_limit) = halt_upon_delay {
            if app.primary.sim_cb.is_none() {
                app.primary.sim_cb = Some(Box::new(FindDelayedIntersections {
                    halt_limit,
                    report_limit: halt_limit,
                    currently_delayed: Vec::new(),
                }));
                // TODO Can we get away with less frequently? Not sure about all the edge cases
                app.primary.sim.set_periodic_callback(Duration::minutes(1));
            } else {
                halt_upon_delay = None;
            }
        }

        Box::new(TimeWarpScreen {
            target,
            wall_time_started: Instant::now(),
            sim_time_started: app.primary.sim.time(),
            halt_upon_delay,
            panel: Panel::new(
                Widget::col(vec![
                    Text::new().draw(ctx).named("text"),
                    Btn::text_bg2("stop now")
                        .build_def(ctx, hotkey(Key::Escape))
                        .centered_horiz(),
                ])
                // hardcoded width avoids jiggle due to text updates
                .force_width(700.0),
            )
            .build(ctx),
        })
    }
}

impl State for TimeWarpScreen {
    fn event(&mut self, ctx: &mut EventCtx, app: &mut App) -> Transition {
        if ctx.input.nonblocking_is_update_event().is_some() {
            ctx.input.use_update_event();
            app.primary.sim.time_limited_step(
                &app.primary.map,
                self.target - app.primary.sim.time(),
                Duration::seconds(0.033),
                &mut app.primary.sim_cb,
            );
            for (t, maybe_i, alert) in app.primary.sim.clear_alerts() {
                // TODO Just the first :(
                return Transition::Replace(PopupMsg::new(
                    ctx,
                    "Alert",
                    vec![format!("At {}, near {:?}, {}", t, maybe_i, alert)],
                ));
            }
            if let Some(ref mut cb) = app.primary.sim_cb {
                let di = cb.downcast_mut::<FindDelayedIntersections>().unwrap();
                if let Some((i, t)) = di.currently_delayed.get(0) {
                    if app.primary.sim.time() - *t > di.halt_limit {
                        let id = ID::Intersection(*i);
                        app.layer =
                            Some(Box::new(crate::layer::traffic::TrafficJams::new(ctx, app)));
                        return Transition::Replace(Warping::new(
                            ctx,
                            id.canonical_point(&app.primary).unwrap(),
                            Some(10.0),
                            Some(id),
                            &mut app.primary,
                        ));
                    }
                }
            }

            let now = app.primary.sim.time();
            let (finished_after, _) = app.primary.sim.num_trips();
            let finished_before = if app.has_prebaked().is_some() {
                let mut cnt = 0;
                for (t, _, _, _) in &app.prebaked().finished_trips {
                    if *t > now {
                        break;
                    }
                    cnt += 1;
                }
                Some(cnt)
            } else {
                None
            };

            let elapsed_sim_time = now - self.sim_time_started;
            let elapsed_wall_time = Duration::realtime_elapsed(self.wall_time_started);
            let txt = Text::from_multiline(vec![
                // I'm covered in shame for not doing this from the start.
                Line("Let's do the time warp again!").small_heading(),
                Line(format!(
                    "{} / {}",
                    now.ampm_tostring(),
                    self.target.ampm_tostring()
                )),
                Line(format!(
                    "Speed: {}x",
                    prettyprint_usize((elapsed_sim_time / elapsed_wall_time) as usize)
                )),
                if let Some(n) = finished_before {
                    // TODO Underline
                    Line(format!(
                        "Finished trips: {} ({} compared to before \"{}\")",
                        prettyprint_usize(finished_after),
                        compare_count(finished_after, n),
                        app.primary.map.get_edits().edits_name,
                    ))
                } else {
                    Line(format!(
                        "Finished trips: {}",
                        prettyprint_usize(finished_after)
                    ))
                },
            ]);

            self.panel.replace(ctx, "text", txt.draw(ctx).named("text"));
        }
        if app.primary.sim.time() == self.target {
            return Transition::Pop;
        }

        match self.panel.event(ctx) {
            Outcome::Clicked(x) => match x.as_ref() {
                "stop now" => {
                    return Transition::Pop;
                }
                _ => unreachable!(),
            },
            _ => {}
        }
        if self.panel.clicked_outside(ctx) {
            return Transition::Pop;
        }

        ctx.request_update(UpdateType::Game);
        Transition::Keep
    }

    fn draw_baselayer(&self) -> DrawBaselayer {
        DrawBaselayer::Custom
    }

    fn draw(&self, g: &mut GfxCtx, app: &App) {
        if app.opts.dont_draw_time_warp {
            g.clear(app.cs.section_bg);
        } else {
            app.draw(
                g,
                DrawOptions::new(),
                &app.primary.sim,
                &ShowEverything::new(),
            );
            State::grey_out_map(g, app);
        }

        self.panel.draw(g);
    }

    fn on_destroy(&mut self, _: &mut EventCtx, app: &mut App) {
        if self.halt_upon_delay.is_some() {
            assert!(app.primary.sim_cb.is_some());
            app.primary.sim_cb = None;
            app.primary.sim.unset_periodic_callback();
        }
    }
}

fn area_under_curve(raw: Vec<(Time, usize)>, width: f64, height: f64) -> Polygon {
    assert!(!raw.is_empty());
    let min_x = Time::START_OF_DAY;
    let min_y = 0;
    let max_x = raw.last().unwrap().0;
    let max_y = raw.iter().max_by_key(|(_, cnt)| *cnt).unwrap().1;

    let mut pts = Vec::new();
    for (t, cnt) in raw {
        pts.push(lttb::DataPoint::new(
            width * (t - min_x) / (max_x - min_x),
            height * (1.0 - (((cnt - min_y) as f64) / ((max_y - min_y) as f64))),
        ));
    }
    let mut downsampled = Vec::new();
    for pt in lttb::lttb(pts, 100) {
        downsampled.push(Pt2D::new(pt.x, pt.y));
    }
    downsampled.push(Pt2D::new(width, height));
    downsampled.push(downsampled[0]);
    Ring::must_new(downsampled).to_polygon()
}

// TODO Maybe color, put in helpers
fn compare_count(after: usize, before: usize) -> String {
    if after == before {
        "+0".to_string()
    } else if after > before {
        format!("+{}", prettyprint_usize(after - before))
    } else {
        format!("-{}", prettyprint_usize(before - after))
    }
}

fn build_jump_to_time_btn(target: Time, ctx: &EventCtx) -> Widget {
    Btn::text_bg2(format!("Jump to {}", target.ampm_tostring()))
        .build(ctx, "jump to time", hotkey(Key::Enter))
        .named("jump to time")
        .centered_horiz()
        .margin_above(16)
}

fn build_jump_to_delay_button(halt_limit: Duration, ctx: &EventCtx) -> Widget {
    Btn::text_bg2(format!("Jump to next {} delay", halt_limit))
        .build(ctx, "jump to delay", hotkey(Key::D))
        .named("jump to delay")
        .centered_horiz()
        .margin_above(16)
}

fn build_jump_to_delay_picker(halt_limit: Duration, ctx: &EventCtx) -> Widget {
    // EZGUI TODO: it'd be nice if we could style the fg color for persistent splits but this needs
    // to be passed into the button builder in init. so either we'd need to make a required
    // argument, or introduce a persistentsplitbuilder, or re-work splitbuilder to allow mutating
    // the color after the fact which requires holding more state to re-invoke the btnbuilder
    PersistentSplit::new(
        ctx,
        "choose delay",
        halt_limit,
        None,
        vec![
            Choice::new("1 minute delay", Duration::minutes(1)),
            Choice::new("2 minute delay", Duration::minutes(2)),
            Choice::new("5 minute delay", Duration::minutes(5)),
            Choice::new("10 minute delay", Duration::minutes(10)),
        ],
    )
    .outline(2.0, Color::WHITE)
    .centered_horiz()
}

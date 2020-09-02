use crate::Config;
use chrono::NaiveTime;
use druid::im::Vector;
use druid::widget::{
    Align, Button, Controller, Flex, Label, List, Padding, Parse, Scroll, Slider, TextBox,
};
use druid::{
    lens, AppDelegate, AppLauncher, Command, Data, DelegateCtx, Env, Event, EventCtx, Lens,
    PlatformError, Selector, Target, Widget, WidgetExt, WindowDesc,
};
use parking_lot::{Condvar, Mutex};
use std::fmt::{self, Display};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Data)]
struct UserTime(#[data(same_fn = "PartialEq::eq")] NaiveTime);

impl FromStr for UserTime {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveTime::from_str(s).map(UserTime).map_err(|_| ())
    }
}
impl Display for UserTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(Debug, Clone, Data)]
struct UserPath(#[data(same_fn = "PartialEq::eq")] PathBuf);

impl FromStr for UserPath {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PathBuf::from_str(s).map(UserPath)
    }
}
impl Display for UserPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.0.display(), f)
    }
}

#[derive(Clone, Data, Lens, Debug)]
struct State {
    initial_fade: Option<f32>,
    fade_in: Option<f32>,
    fade_out: Option<f32>,
    update_interval: Option<f32>,
    anchor_time: Option<UserTime>,
    dir: Option<UserPath>,
    gain: f64,
    times: Vector<(Option<UserTime>, Option<UserPath>)>,
}

impl State {
    fn from_config(mut config: Config) -> Self {
        Self {
            initial_fade: config.initial_fade,
            fade_in: config.fade_in,
            fade_out: config.fade_out,
            update_interval: config.update_interval,
            anchor_time: config.anchor_time.map(|t| UserTime(t)),
            dir: config.dir.map(|d| UserPath(d)),
            gain: 1.0,
            times: config
                .times
                .remove("normal")
                .unwrap()
                .into_iter()
                .map(|(time, path)| (Some(UserTime(time)), Some(UserPath(path))))
                .collect(),
        }
    }
    fn to_config(&self) -> Config {
        let mut times = std::collections::HashMap::new();
        times.insert(
            "normal".to_string(),
            self.times
                .iter()
                .filter_map(|(k, v)| {
                    k.as_ref()
                        .and_then(|k| v.as_ref().map(|v| (k.0.clone(), v.0.clone())))
                })
                .collect(),
        );
        Config {
            initial_fade: self.initial_fade,
            fade_in: self.fade_in,
            fade_out: self.fade_out,
            update_interval: self.update_interval,
            anchor_time: self.anchor_time.as_ref().map(|t| t.0.clone()),
            dir: self.dir.as_ref().map(|d| d.0.clone()),
            times,
        }
    }
}

pub(crate) fn gui_main(
    mc: Arc<(Mutex<Config>, Condvar)>,
    gain: Arc<AtomicU32>,
    init_config: Config,
) -> Result<(), PlatformError> {
    let state = State::from_config(init_config);
    let delegate = Delegate { mc, gain };
    AppLauncher::with_window(WindowDesc::new(build_ui))
        .delegate(delegate)
        .launch(state)?;
    Ok(())
}

const APPLY_CONFIG: Selector<()> = Selector::new("hourly-music.apply-config");
const ADJUST_GAIN: Selector<f64> = Selector::new("hourly-music.adjust-gain");

struct Delegate {
    mc: Arc<(Mutex<Config>, Condvar)>,
    gain: Arc<AtomicU32>,
}

impl AppDelegate<State> for Delegate {
    fn command(
        &mut self,
        ctx: &mut DelegateCtx,
        target: Target,
        cmd: &Command,
        data: &mut State,
        env: &Env,
    ) -> bool {
        if let Some(()) = cmd.get(APPLY_CONFIG) {
            {
                *self.mc.0.lock() = data.to_config();
            }
            self.mc.1.notify_all();
            false
        } else if let Some(gain) = cmd.get(ADJUST_GAIN) {
            self.gain.store((*gain as f32).to_bits(), Ordering::Relaxed);
            self.mc.1.notify_all();
            false
        } else {
            true
        }
    }
}

fn build_ui() -> impl Widget<State> {
    Flex::column()
        .with_child(
            Flex::column()
                .with_child(
                    Flex::row()
                        .with_child(Label::new("dir"))
                        .with_flex_child(path_entry().expand_width().lens(State::dir), 1.0),
                )
                .with_child(
                    Flex::row()
                        .with_child(Label::new("fade_in"))
                        .with_flex_child(delay_entry().expand_width().lens(State::fade_in), 1.0),
                )
                .with_child(
                    Flex::row()
                        .with_child(Label::new("fade_out"))
                        .with_flex_child(delay_entry().expand_width().lens(State::fade_out), 1.0),
                )
                .with_child(
                    Flex::row()
                        .with_child(Label::new("initial_fade"))
                        .with_flex_child(
                            delay_entry().expand_width().lens(State::initial_fade),
                            1.0,
                        ),
                )
                .with_child(
                    Flex::row()
                        .with_child(Label::new("update_interval"))
                        .with_flex_child(
                            delay_entry().expand_width().lens(State::update_interval),
                            1.0,
                        ),
                )
                .with_child(
                    Flex::row()
                        .with_child(Label::new("anchor_time"))
                        .with_flex_child(time_entry().expand_width().lens(State::anchor_time), 1.0),
                ),
        )
        .with_child(
            Slider::new()
                .with_range(0.0, 1.5)
                .controller(GainController)
                .lens(State::gain)
                .expand_width(),
        )
        .with_flex_child(
            Scroll::new(List::new(list_element))
                .vertical()
                .lens(State::times),
            1.0,
        )
        .with_child(Button::new("Apply").on_click(|ctx, t, env| {
            ctx.submit_command(APPLY_CONFIG, Target::Global);
        }))
}

struct GainController;

impl<W: Widget<f64>> Controller<f64, W> for GainController {
    fn event(
        &mut self,
        child: &mut W,
        ctx: &mut EventCtx,
        event: &Event,
        data: &mut f64,
        env: &Env,
    ) {
        let gain = *data;
        child.event(ctx, event, data, env);
        if gain != *data {
            ctx.submit_command(ADJUST_GAIN.with(*data), Target::Global)
        }
    }
}

fn time_entry() -> impl Widget<Option<UserTime>> {
    Parse::new(TextBox::new())
}

fn path_entry() -> impl Widget<Option<UserPath>> {
    Parse::new(TextBox::new())
}

fn delay_entry() -> impl Widget<Option<f32>> {
    Parse::new(TextBox::new())
}

fn list_element() -> impl Widget<(Option<UserTime>, Option<UserPath>)> {
    Flex::row()
        .with_child(time_entry().lens(lens!((_, _), 0)))
        .with_flex_child(path_entry().lens(lens!((_, _), 1)), 1.0)
        .expand_width()
}

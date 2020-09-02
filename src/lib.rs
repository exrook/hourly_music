use chrono::{Local, NaiveTime};
#[cfg(feature = "gui")]
use parking_lot::{Condvar, Mutex};
use rodio::{self, Sink, Source};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
#[cfg(feature = "gui")]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "gui")]
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(feature = "gui")]
mod gui;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    initial_fade: Option<f32>,
    fade_in: Option<f32>,
    fade_out: Option<f32>,
    update_interval: Option<f32>,
    anchor_time: Option<NaiveTime>,
    dir: Option<PathBuf>,
    times: HashMap<String, BTreeMap<NaiveTime, PathBuf>>,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    initial_fade: Duration,
    fade_in: Duration,
    fade_out: Duration,
    update_interval: Duration,
    anchor_time: Option<NaiveTime>,
    dir: Option<PathBuf>,
    times: HashMap<String, BTreeMap<NaiveTime, PathBuf>>,
}

impl Config {
    #[cfg(target_os = "android")]
    fn load() -> LoadedConfig {
        use std::ffi::OsStr;
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        let data_path = Path::new(OsStr::from_bytes(
            ndk_glue::native_activity().external_data_path().to_bytes(),
        ));
        let config_path = data_path.join("config.toml");
        if !config_path.exists() {
            let default_config = include_bytes!("../example-config.toml");
            fs::write(&config_path, &default_config[..]).expect("Unable to write config file");
        }
        let mut config = Self::load_common(config_path);
        config.dir = Some(data_path.into());
        config
    }
    #[cfg(not(target_os = "android"))]
    fn load() -> LoadedConfig {
        Self::load_common("config.toml")
    }
    fn load_common<P: AsRef<Path>>(path: P) -> LoadedConfig {
        let config: Config = toml::from_slice(&fs::read(path).expect("Unable to load config file"))
            .expect("Invalid config file");
        config.to_loaded()
    }
    fn save<P: AsRef<Path>>(config: LoadedConfig, path: P) {
        let config = Config::from_loaded(config);
        let out = toml::to_vec(&config).expect("Unable to serialize config file");
        fs::write(path, out).expect("Unable to write config file");
    }
    fn to_loaded(self) -> LoadedConfig {
        let fade_in = Duration::from_secs_f32(self.fade_in.unwrap_or(5.0));
        LoadedConfig {
            fade_in,
            fade_out: Duration::from_secs_f32(self.fade_out.unwrap_or(5.0)),
            initial_fade: self
                .initial_fade
                .map(Duration::from_secs_f32)
                .unwrap_or(fade_in),
            update_interval: Duration::from_secs_f32(self.update_interval.unwrap_or(60.0)),
            anchor_time: self.anchor_time,
            dir: self.dir,
            times: self.times,
        }
    }
    fn from_loaded(config: LoadedConfig) -> Self {
        Config {
            fade_in: Some(config.fade_in.as_secs_f32()),
            fade_out: Some(config.fade_out.as_secs_f32()),
            initial_fade: Some(config.initial_fade.as_secs_f32()),
            update_interval: Some(config.update_interval.as_secs_f32()),
            anchor_time: config.anchor_time,
            dir: config.dir,
            times: config.times,
        }
    }
}

impl LoadedConfig {
    fn current_song(&self, time: NaiveTime) -> Cow<PathBuf> {
        let path = self.times["normal"]
            .range(..time)
            .last()
            .map(|(_k, v)| v)
            .or_else(|| self.times["normal"].values().last())
            .unwrap();
        self.dir
            .as_ref()
            .map(|dir| Cow::Owned(dir.clone().join(path)))
            .unwrap_or(Cow::Borrowed(path))
    }
    fn next_song(&self, time: NaiveTime) -> (&NaiveTime, Cow<PathBuf>) {
        let (time, path) = self.times["normal"]
            .range(time..)
            .next()
            .or_else(|| self.times["normal"].iter().next())
            .unwrap();
        let path = self
            .dir
            .as_ref()
            .map(|dir| Cow::Owned(dir.clone().join(path)))
            .unwrap_or(Cow::Borrowed(path));
        (time, path)
    }
}

#[cfg(not(feature = "gui"))]
struct Sleeper {
    config: LoadedConfig,
    anchor_time: NaiveTime,
}

#[cfg(feature = "gui")]
struct Sleeper {
    condvar: std::sync::Arc<(Mutex<Config>, Condvar)>,
    config: LoadedConfig,
    anchor_time: NaiveTime,
    gain: Arc<AtomicU32>,
}

#[cfg(not(feature = "gui"))]
impl Sleeper {
    fn new(config: LoadedConfig, anchor_time: NaiveTime) -> Self {
        Self {
            config,
            anchor_time,
        }
    }
    fn sleep(&mut self, now: chrono::DateTime<Local>) {
        let anchor_time = self.config.anchor_time.unwrap_or(self.anchor_time);
        thread::sleep(calculate_sleep_duration(
            anchor_time,
            now,
            self.config.update_interval,
        ))
    }
}

#[cfg(feature = "gui")]
impl Sleeper {
    fn new(config: LoadedConfig, anchor_time: NaiveTime) -> Self {
        let pair = Arc::new((
            Mutex::new(Config::from_loaded(config.clone())),
            Condvar::new(),
        ));
        let pair2 = pair.clone();
        let config2 = config.clone();
        let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let gain2 = gain.clone();

        thread::spawn(move || {
            //let &(ref lock, ref condvar) = &*pair2;
            gui::gui_main(pair2, gain2, Config::from_loaded(config2)).unwrap();
        });

        Self {
            condvar: pair,
            config,
            anchor_time,
            gain,
        }
    }
    fn sleep(&mut self, now: chrono::DateTime<Local>) {
        let anchor_time = self.config.anchor_time.unwrap_or(self.anchor_time);
        let &(ref mutex, ref condvar) = &*self.condvar;
        let mut new_config = mutex.lock();
        if !condvar
            .wait_for(
                &mut new_config,
                calculate_sleep_duration(anchor_time, now, self.config.update_interval),
            )
            .timed_out()
        {
            self.config = new_config.clone().to_loaded()
        }
    }
    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

fn calculate_sleep_duration(
    anchor_time: NaiveTime,
    now: chrono::DateTime<Local>,
    update_interval: Duration,
) -> Duration {
    // Don't let our update interval drift
    let remainder = (now.time() - anchor_time)
        .to_std()
        .ok()
        .map(|r| r.as_micros() % update_interval.as_micros())
        .and_then(|m| m.try_into().ok())
        .map(Duration::from_micros)
        .map(|r| update_interval - r);
    remainder.unwrap_or(update_interval)
}

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    let config = Config::load();

    let start = Local::now();
    let mut sleeper = Sleeper::new(config, start.time());
    let mut current_song_path = sleeper.config.current_song(start.time()).into_owned();
    println!("Starting with {:?}", current_song_path);

    let (_stream, stream_handle) =
        rodio::OutputStream::try_default().expect("Unable to open audio output device");
    let mut current_song = Sink::try_new(&stream_handle).unwrap();
    current_song.append(
        rodio::Decoder::new_looped(
            File::open(&current_song_path)
                .expect("Error opening song file, check if the file path is correct"),
        )
        .expect("Error parsing audio file")
        .fade_in(sleeper.config.initial_fade),
    );

    loop {
        let now = Local::now();

        #[cfg(feature = "gui")]
        current_song.set_volume(sleeper.gain());

        let new_path = sleeper.config.current_song(now.time()).into_owned();
        if new_path != current_song_path {
            println!("Changing songs, new song: {:?}", new_path);
            current_song_path = new_path;

            fade_out(current_song, sleeper.config.fade_out);
            current_song = Sink::try_new(&stream_handle).unwrap();
            current_song.append(
                rodio::Decoder::new_looped(
                    File::open(&current_song_path)
                        .expect("Error opening song file, check if the file path is correct"),
                )
                .expect("Error parsing audio file")
                .fade_in(sleeper.config.fade_in),
            );
        }
        sleeper.sleep(now);
    }
}

fn fade_out(sink: Sink, duration: Duration) {
    thread::spawn(move || {
        let now = Instant::now();
        loop {
            let elapsed = now.elapsed();
            if elapsed > duration {
                break;
            }
            let volume = (duration.as_secs_f32() - elapsed.as_secs_f32()) / duration.as_secs_f32();
            sink.set_volume(volume.max(0.0));
            thread::sleep(duration / 100);
        }
    });
}

use chrono::{Local, NaiveTime};
use rodio::{self, Sink, Source};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
struct Config {
    initial_fade: Option<f32>,
    fade_in: Option<f32>,
    fade_out: Option<f32>,
    update_interval: Option<f32>,
    anchor_time: Option<NaiveTime>,
    reload_config: Option<bool>,
    dir: Option<PathBuf>,
    times: HashMap<String, BTreeMap<NaiveTime, PathBuf>>,
}

#[derive(Debug)]
struct LoadedConfig {
    initial_fade: Duration,
    fade_in: Duration,
    fade_out: Duration,
    update_interval: Duration,
    anchor_time: Option<NaiveTime>,
    dir: Option<PathBuf>,
    times: HashMap<String, BTreeMap<NaiveTime, PathBuf>>,
}

impl Config {
    fn load<P: AsRef<Path>>(path: P) -> LoadedConfig {
        let config: Config = toml::from_slice(&fs::read(path).expect("Unable to load config file"))
            .expect("Invalid config file");
        let fade_in = Duration::from_secs_f32(config.fade_in.unwrap_or(5.0));
        LoadedConfig {
            fade_in,
            fade_out: Duration::from_secs_f32(config.fade_out.unwrap_or(5.0)),
            initial_fade: config
                .initial_fade
                .map(Duration::from_secs_f32)
                .unwrap_or(fade_in),
            update_interval: Duration::from_secs_f32(config.update_interval.unwrap_or(60.0)),
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

pub fn main() {
    let config = Config::load("config.toml");

    let start = Local::now();
    let mut current_song_path = config.current_song(start.time());
    println!("Starting with {:?}", current_song_path);

    let (_stream, stream_handle) =
        rodio::OutputStream::try_default().expect("Unable to open audio output device");
    let mut current_song = Sink::try_new(&stream_handle).unwrap();
    current_song.append(
        rodio::Decoder::new_looped(
            File::open(current_song_path.as_ref())
                .expect("Error opening song file, check if the file path is correct"),
        )
        .expect("Error parsing audio file")
        .fade_in(config.initial_fade),
    );
    let anchor_time = config.anchor_time.unwrap_or(start.time());

    loop {
        let now = Local::now();

        let new_path = config.current_song(now.time());
        if new_path != current_song_path {
            println!("Changing songs, new song: {:?}", new_path);
            current_song_path = new_path;

            fade_out(current_song, config.fade_out);
            current_song = Sink::try_new(&stream_handle).unwrap();
            current_song.append(
                rodio::Decoder::new_looped(
                    File::open(current_song_path.as_ref())
                        .expect("Error opening song file, check if the file path is correct"),
                )
                .expect("Error parsing audio file")
                .fade_in(config.fade_in),
            );
        }
        println!("E");
        // Don't let our update interval drift
        let remainder = (now.time() - anchor_time)
            .to_std()
            .ok()
            .map(|r| r.as_micros() % config.update_interval.as_micros())
            .and_then(|m| m.try_into().ok())
            .map(Duration::from_micros)
            .map(|r| config.update_interval - r);
        dbg!(remainder);
        dbg!(now.time(), anchor_time);
        thread::sleep(remainder.unwrap_or(config.update_interval));
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

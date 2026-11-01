#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------- helpers

fn ffmpeg_path(app: &AppHandle) -> PathBuf {
    let mut c: Vec<PathBuf> = vec![];
    if let Ok(r) = app.path().resource_dir() {
        c.push(r.join("bin").join("ffmpeg.exe"));
        c.push(r.join("ffmpeg.exe"));
    }
    if let Ok(e) = std::env::current_exe() {
        if let Some(d) = e.parent() {
            c.push(d.join("bin").join("ffmpeg.exe"));
            c.push(d.join("ffmpeg.exe"));
        }
    }
    c.into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// A Command that never flashes a console window on Windows.
fn cmd(p: &PathBuf) -> Command {
    let mut c = Command::new(p);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn key(path: &str, tag: &str) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:x}_{}", h.finish(), tag)
}

fn cache_dir(app: &AppHandle, sub: &str) -> Result<PathBuf, String> {
    let d = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join(sub);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn has_audio(ff: &PathBuf, p: &str) -> bool {
    cmd(ff)
        .args(["-hide_banner", "-i", p])
        .stdin(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stderr).contains("Audio:"))
        .unwrap_or(false)
}

fn add(args: &mut Vec<String>, v: &[&str]) {
    args.extend(v.iter().map(|s| s.to_string()));
}

fn atempo(mut s: f64) -> String {
    let mut v: Vec<String> = vec![];
    while s > 2.0 {
        v.push("atempo=2.0".into());
        s /= 2.0;
    }
    while s < 0.5 {
        v.push("atempo=0.5".into());
        s /= 0.5;
    }
    v.push(format!("atempo={:.5}", s));
    v.join(",")
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn ffmpeg_ok(app: AppHandle) -> bool {
    cmd(&ffmpeg_path(&app))
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Low-resolution, short-GOP proxy: smooth preview + instant hover scrubbing,
/// and a fallback for codecs the WebView cannot play.
#[tauri::command]
async fn make_proxy(app: AppHandle, path: String, height: u32) -> Result<String, String> {
    let ff = ffmpeg_path(&app);
    let dir = cache_dir(&app, "proxies")?;
    let out = dir.join(format!("{}.mp4", key(&path, &height.to_string())));
    if out.exists() {
        return Ok(out.to_string_lossy().to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let tmp = dir.join(format!("{}.part.mp4", key(&path, &height.to_string())));
        let vf = format!("scale=-2:'min({height},ih)'");
        let o = cmd(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error", "-i", path.as_str(),
                "-map", "0:v:0", "-map", "0:a:0?",
                "-vf", vf.as_str(),
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "27",
                "-g", "8", "-keyint_min", "8", "-sc_threshold", "0",
                "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k",
                "-movflags", "+faststart",
            ])
            .arg(&tmp)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).to_string());
        }
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
        Ok(out.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn make_thumb(app: AppHandle, path: String) -> Result<String, String> {
    let ff = ffmpeg_path(&app);
    let dir = cache_dir(&app, "thumbs")?;
    let out = dir.join(format!("{}.jpg", key(&path, "t")));
    if out.exists() {
        return Ok(out.to_string_lossy().to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        for ss in ["0.5", "0"] {
            let _ = cmd(&ff)
                .args([
                    "-y", "-hide_banner", "-loglevel", "error", "-ss", ss, "-i", path.as_str(),
                    "-frames:v", "1", "-vf", "scale=-2:90",
                ])
                .arg(&out)
                .stdin(Stdio::null())
                .output();
            if out.exists() {
                return Ok(out.to_string_lossy().to_string());
            }
        }
        Err("thumbnail failed".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- export

#[derive(Deserialize)]
struct Fx {
    brightness: f64,
    contrast: f64,
    saturation: f64,
    blur: f64,
    filter: String,
    flip: bool,
}

#[derive(Deserialize)]
struct Mask {
    on: bool,
    shape: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    blur: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClipJob {
    path: String,
    inn: f64,
    out: f64,
    speed: f64,
    volume: f64,
    fx: Fx,
    mask: Mask,
    trans: String,
    trans_dur: f64,
    text_png: Option<String>,
}

#[derive(Deserialize)]
struct Job {
    clips: Vec<ClipJob>,
    width: u32,
    height: u32,
    fps: u32,
    crf: u32,
    codec: String,
    preset: String,
    output: String,
}

const TRANSITIONS: &[&str] = &[
    "fade", "dissolve", "fadeblack", "wipeleft", "wiperight", "slideleft", "slideright",
    "circleopen", "circleclose", "pixelize", "radial", "zoomin", "smoothleft",
];

/// Builds the full ffmpeg argument list. Returns (args, total_duration_seconds).
fn build(ff: &PathBuf, job: &Job) -> Result<(Vec<String>, f64), String> {
    let n = job.clips.len();
    if n == 0 {
        return Err("لا توجد مقاطع للتصدير".into());
    }
    let (w, h) = (job.width as i64, job.height as i64);
    let fps = job.fps.max(1);
    let hscale = h as f64 / 1080.0;

    let mut args: Vec<String> = vec![];
    let mut fc: Vec<String> = vec![];
    let mut idx = 0usize;
    let mut lens: Vec<f64> = vec![];

    for (i, c) in job.clips.iter().enumerate() {
        let sp = c.speed.clamp(0.05, 16.0);
        let dur = (c.out - c.inn).max(0.05);
        let l = dur / sp;
        lens.push(l);

        // Input-side seeking: fast even for long sources.
        args.extend([
            "-ss".into(), format!("{:.3}", c.inn),
            "-t".into(), format!("{:.3}", dur),
            "-i".into(), c.path.clone(),
        ]);
        let vi = idx;
        idx += 1;

        // ---- video: speed, fps, fit into canvas (letterbox), then effects
        let mut chain = format!(
            "[{vi}:v]setpts=(PTS-STARTPTS)/{sp:.5},fps={fps},\
             scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black,setsar=1,format=yuv420p"
        );
        let f = &c.fx;
        if f.flip {
            chain.push_str(",hflip");
        }
        if f.brightness != 0.0 || f.contrast != 0.0 || f.saturation != 0.0 {
            chain.push_str(&format!(
                ",eq=brightness={:.3}:contrast={:.3}:saturation={:.3}",
                f.brightness / 250.0,
                (1.0 + f.contrast / 100.0).max(0.0),
                (1.0 + f.saturation / 100.0).max(0.0)
            ));
        }
        match f.filter.as_str() {
            "bw" => chain.push_str(",hue=s=0"),
            "sepia" => chain.push_str(
                ",colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131",
            ),
            "warm" => chain.push_str(",colorbalance=rs=.12:bs=-.12"),
            "cool" => chain.push_str(",colorbalance=rs=-.12:bs=.12"),
            "vignette" => chain.push_str(",vignette=PI/4"),
            "sharpen" => chain.push_str(",unsharp=5:5:1.0:5:5:0.0"),
            _ => {}
        }
        if f.blur > 0.05 {
            chain.push_str(&format!(",gblur=sigma={:.2}", f.blur * hscale));
        }
        let mut cur = format!("b{i}");
        fc.push(format!("{chain}[{cur}]"));

        // ---- mask: blur only a selected region (rectangle or ellipse)
        let m = &c.mask;
        if m.on && m.blur > 0.05 {
            let cx = ((m.x * w as f64).round() as i64).clamp(0, w - 2) & !1;
            let cy = ((m.y * h as f64).round() as i64).clamp(0, h - 2) & !1;
            let cw = ((m.w * w as f64).round() as i64).clamp(2, w - cx) & !1;
            let ch = ((m.h * h as f64).round() as i64).clamp(2, h - cy) & !1;
            let sig = m.blur * hscale;
            let mut sub = format!("[mb{i}]crop={cw}:{ch}:{cx}:{cy},gblur=sigma={sig:.2}");
            if m.shape == "ellipse" {
                sub.push_str(
                    ",format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':\
                     a='if(lte(pow((2*X-W)/W,2)+pow((2*Y-H)/H,2),1),255,0)'",
                );
            }
            fc.push(format!("[{cur}]split[ma{i}][mb{i}]"));
            fc.push(format!("{sub}[mc{i}]"));
            fc.push(format!("[ma{i}][mc{i}]overlay={cx}:{cy}:format=auto[m{i}]"));
            cur = format!("m{i}");
        }

        // ---- text: rendered by the UI (correct Arabic shaping) as a transparent PNG
        if let Some(png) = &c.text_png {
            let data = png.split(',').last().unwrap_or("");
            let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
            let p = std::env::temp_dir().join(format!("rustcut_text_{i}.png"));
            std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
            args.extend([
                "-loop".into(), "1".into(),
                "-framerate".into(), fps.to_string(),
                "-t".into(), format!("{:.3}", l),
                "-i".into(), p.to_string_lossy().to_string(),
            ]);
            let ti = idx;
            idx += 1;
            fc.push(format!("[{cur}][{ti}:v]overlay=0:0:shortest=1:format=auto[t{i}]"));
            cur = format!("t{i}");
        }
        fc.push(format!("[{cur}]format=yuv420p[v{i}]"));

        // ---- audio: speed (pitch preserved), volume; silence if the file has none
        let a = if has_audio(ff, &c.path) {
            format!(
                "[{vi}:a]asetpts=PTS-STARTPTS,{},volume={:.3},aresample=48000,\
                 aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]",
                atempo(sp),
                c.volume
            )
        } else {
            format!(
                "anullsrc=r=48000:cl=stereo,atrim=duration={:.3},asetpts=PTS-STARTPTS,\
                 aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]",
                l
            )
        };
        fc.push(a);
    }

    // ---- join clips: xfade where a transition is set, plain concat otherwise
    let mut cv = "v0".to_string();
    let mut ca = "a0".to_string();
    let mut clen = lens[0];
    for i in 1..n {
        let p = &job.clips[i - 1];
        let d = p.trans_dur.min(clen * 0.5).min(lens[i] * 0.5);
        if TRANSITIONS.contains(&p.trans.as_str()) && d >= 0.05 {
            fc.push(format!(
                "[{cv}][v{i}]xfade=transition={}:duration={d:.3}:offset={:.3},fps={fps}[xv{i}]",
                p.trans,
                clen - d
            ));
            fc.push(format!("[{ca}][a{i}]acrossfade=d={d:.3}[xa{i}]"));
            clen += lens[i] - d;
        } else {
            fc.push(format!("[{cv}][v{i}]concat=n=2:v=1:a=0,fps={fps}[xv{i}]"));
            fc.push(format!("[{ca}][a{i}]concat=n=2:v=0:a=1[xa{i}]"));
            clen += lens[i];
        }
        cv = format!("xv{i}");
        ca = format!("xa{i}");
    }

    args.extend([
        "-filter_complex".into(), fc.join(";"),
        "-map".into(), format!("[{cv}]"),
        "-map".into(), format!("[{ca}]"),
    ]);

    let preset = match job.preset.as_str() {
        "ultrafast" | "superfast" | "veryfast" | "faster" | "fast" | "medium" | "slow"
        | "slower" => job.preset.as_str(),
        _ => "medium",
    };
    let crf = job.crf.clamp(0, 51).to_string();
    match job.codec.as_str() {
        "h265" => add(&mut args, &[
            "-c:v", "libx265", "-preset", preset, "-crf", crf.as_str(),
            "-tag:v", "hvc1", "-pix_fmt", "yuv420p",
        ]),
        "nvenc" => add(&mut args, &[
            "-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", crf.as_str(),
            "-b:v", "0", "-pix_fmt", "yuv420p",
        ]),
        _ => add(&mut args, &[
            "-c:v", "libx264", "-preset", preset, "-crf", crf.as_str(), "-pix_fmt", "yuv420p",
        ]),
    }
    add(&mut args, &["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]);
    Ok((args, clen))
}

#[tauri::command]
async fn export_video(app: AppHandle, job: Job) -> Result<String, String> {
    let ff = ffmpeg_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let (rest, total) = build(&ff, &job)?;
        let mut a: Vec<String> = vec![];
        add(&mut a, &["-y", "-hide_banner", "-nostats", "-progress", "pipe:1"]);
        a.extend(rest);
        a.push(job.output.clone());

        let mut child = cmd(&ff)
            .args(&a)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("تعذر تشغيل ffmpeg: {e}"))?;

        let mut err = child.stderr.take().ok_or("no stderr")?;
        let log_thread = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });

        let out = child.stdout.take().ok_or("no stdout")?;
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let pct = (us / 1e6 / total * 100.0).clamp(0.0, 100.0);
                    let _ = app.emit("export-progress", pct);
                }
            }
        }

        let st = child.wait().map_err(|e| e.to_string())?;
        let log = log_thread.join().unwrap_or_default();
        if st.success() {
            let _ = app.emit("export-progress", 100.0);
            Ok(job.output.clone())
        } else {
            let tail: Vec<&str> = log.lines().rev().take(14).collect();
            Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            ffmpeg_ok,
            make_proxy,
            make_thumb,
            export_video
        ])
        .run(tauri::generate_context!())
        .expect("error while running RustCut");
}

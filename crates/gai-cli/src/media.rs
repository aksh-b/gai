//! `gai speak`, `gai transcribe` and `gai image`: the media models from the
//! command line.

use std::path::{Path, PathBuf};

use gai::Gai;
use gai::llm::{ImageRequest, SpeechRequest, TranscribeRequest, extension_for};

use crate::style::{dim, green};

#[derive(clap::Args)]
pub struct SpeakArgs {
    /// What to say. Reads stdin when omitted or `-`.
    pub text: Vec<String>,
    /// Text-to-speech model (default: [media] speech)
    #[arg(long, short)]
    pub model: Option<String>,
    #[arg(long, short)]
    pub voice: Option<String>,
    /// mp3, wav, opus...
    #[arg(long, short, default_value = "mp3")]
    pub format: String,
    /// Output file (default: speech.<format>)
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct TranscribeArgs {
    /// An audio file
    pub file: PathBuf,
    /// Speech-to-text model (default: [media] transcription)
    #[arg(long, short)]
    pub model: Option<String>,
    /// Language of the speech (ISO-639-1, e.g. en)
    #[arg(long, short)]
    pub language: Option<String>,
    /// Label speakers and times, where the model can
    #[arg(long)]
    pub speakers: bool,
}

#[derive(clap::Args)]
pub struct NotesArgs {
    /// A recording: audio or video
    pub file: PathBuf,
    /// meeting (default), audio (speakers, sentiment) or video (default
    /// for video files)
    #[arg(long, short)]
    pub kind: Option<String>,
    /// Notes template: general, standup, one_on_one, sales_call,
    /// interview, retro, lecture
    #[arg(long, short)]
    pub template: Option<String>,
    /// What else is known: agenda, attendees, names to spell right
    #[arg(long, short)]
    pub context: Option<String>,
    /// Language of the speech (ISO-639-1)
    #[arg(long, short)]
    pub language: Option<String>,
    /// Write the notes here (default: print them)
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ImageArgs {
    /// What to draw
    pub prompt: Vec<String>,
    /// Image model (default: [media] image)
    #[arg(long, short)]
    pub model: Option<String>,
    /// How many images
    #[arg(long, short, default_value_t = 1)]
    pub n: u32,
    /// e.g. 1024x1024
    #[arg(long)]
    pub size: Option<String>,
    #[arg(long)]
    pub quality: Option<String>,
    /// Output file; numbered when there are several (default: image.<ext>)
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

fn model(given: Option<String>, default: &Option<String>, what: &str) -> anyhow::Result<String> {
    given
        .or_else(|| default.clone())
        .ok_or_else(|| anyhow::anyhow!("give --model, or set {what} under [media] in gai.toml"))
}

fn cost_note(cost: Option<f64>) -> String {
    cost.map(|c| format!(" · ${c:.4}")).unwrap_or_default()
}

pub async fn speak(gai: &Gai, a: SpeakArgs) -> anyhow::Result<()> {
    let text = crate::run::read_prompt(&a.text)?;
    let media = &gai.config().media;
    let m = gai
        .models()
        .speech(&model(a.model, &media.speech, "speech")?)?;
    let s = m
        .speak(SpeechRequest {
            text,
            voice: a.voice.or_else(|| media.voice.clone()),
            format: Some(a.format.clone()),
            ..Default::default()
        })
        .await?;
    let out = a
        .out
        .unwrap_or_else(|| PathBuf::from(format!("speech.{}", extension_for(&s.media_type))));
    std::fs::write(&out, &s.audio)?;
    println!(
        "{} {} {}",
        green("saved"),
        out.display(),
        dim(&format!(
            "({} bytes, {}{})",
            s.audio.len(),
            m.info().id,
            cost_note(m.info().media_cost(&s.usage))
        ))
    );
    Ok(())
}

fn audio_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "webm" => "audio/webm",
        "m4a" | "mp4" => "audio/mp4",
        "flac" => "audio/flac",
        _ => "audio/mpeg",
    }
}

pub async fn transcribe(gai: &Gai, a: TranscribeArgs) -> anyhow::Result<()> {
    let m = gai.models().transcription(&model(
        a.model,
        &gai.config().media.transcription,
        "transcription",
    )?)?;
    let audio = std::fs::read(&a.file)?;
    let t = m
        .transcribe(TranscribeRequest {
            audio,
            media_type: audio_type(&a.file).into(),
            filename: a.file.file_name().map(|n| n.to_string_lossy().into_owned()),
            language: a.language,
            prompt: None,
            diarize: a.speakers,
        })
        .await?;
    if a.speakers || t.has_speakers() {
        println!("{}", t.timed_text());
    } else {
        println!("{}", t.text);
    }
    eprintln!(
        "{}",
        dim(&format!(
            "{}{}{}",
            m.info().id,
            t.duration_secs
                .map(|d| format!(" · {d:.1}s"))
                .unwrap_or_default(),
            cost_note(m.info().media_cost(&t.usage))
        ))
    );
    Ok(())
}

pub async fn image(gai: &Gai, a: ImageArgs) -> anyhow::Result<()> {
    let prompt = crate::run::read_prompt(&a.prompt)?;
    let m = gai
        .models()
        .image(&model(a.model, &gai.config().media.image, "image")?)?;
    let resp = m
        .generate(ImageRequest {
            prompt,
            size: a.size,
            n: Some(a.n.max(1)),
            quality: a.quality,
        })
        .await?;
    let many = resp.images.len() > 1;
    for (i, img) in resp.images.iter().enumerate() {
        let ext = extension_for(&img.media_type);
        let path = match (&a.out, many) {
            (Some(p), false) => p.clone(),
            (Some(p), true) => {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                p.with_file_name(format!("{stem}-{}.{ext}", i + 1))
            }
            (None, false) => PathBuf::from(format!("image.{ext}")),
            (None, true) => PathBuf::from(format!("image-{}.{ext}", i + 1)),
        };
        std::fs::write(&path, &img.data)?;
        println!("{} {}", green("saved"), path.display());
        if let Some(r) = &img.revised_prompt {
            eprintln!("{}", dim(&format!("  drawn from: {r}")));
        }
    }
    eprintln!(
        "{}",
        dim(&format!(
            "{}{}",
            m.info().id,
            cost_note(m.info().media_cost(&resp.usage))
        ))
    );
    Ok(())
}

pub async fn notes(gai: &Gai, a: NotesArgs) -> anyhow::Result<()> {
    use gai::media::transcribe::{MediaInput, media_type_for};
    let name = a
        .file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let input = MediaInput {
        data: std::fs::read(&a.file)?,
        media_type: media_type_for(&name).to_owned(),
        name: Some(name.clone()),
    };
    let kind = a
        .kind
        .unwrap_or_else(|| if input.is_video() { "video" } else { "meeting" }.into());
    let options = gai::media::RecordingOptions {
        language: a.language,
        template: a.template,
        context: a.context,
        ..Default::default()
    };
    let recordings = gai.recordings().await?;
    let meter = gai::media::Meter::new(Some(gai.runtime().await?.store().clone()), None);
    eprintln!("{}", dim(&format!("Processing {name} as a {kind}...")));
    let result = recordings.analyze(&kind, &input, &options, &meter).await?;
    let title = result
        .notes
        .as_ref()
        .map(|n| n.title.clone())
        .or_else(|| result.video.as_ref().map(|v| v.title.clone()))
        .unwrap_or(name);
    let saved = recordings
        .save(&kind, &title, None, &options, &result)
        .await?;
    match a.out {
        Some(out) => {
            std::fs::write(&out, &result.markdown)?;
            println!("{} {}", green("wrote"), out.display());
        }
        None => println!("{}", result.markdown),
    }
    eprintln!(
        "{}",
        dim(&format!(
            "{} · ${:.4} · saved as {}",
            result.models.transcription.as_deref().unwrap_or("?"),
            result.cost,
            saved.id
        ))
    );
    Ok(())
}

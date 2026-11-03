//! Decode an audio/video file to 16 kHz mono s16 PCM (file mode).

use crate::audio::resample::Resampler;
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const TARGET_RATE: u32 = 16_000;
/// Bounds memory: 2 h of 16 kHz s16 is ~230 MB, plus the WAV copy.
const MAX_DECODE_SECONDS: usize = 2 * 60 * 60;

/// Average interleaved frames down to one channel.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Minimal PCM s16le RIFF/WAVE file.
pub fn encode_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Decode the default audio track of `path` to 16 kHz mono s16. Corrupt
/// packets are skipped; an unreadable container or codec is an error.
pub fn decode_file_to_pcm16k(path: &Path) -> Result<Vec<i16>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("Unsupported or unreadable media file: {e}"))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or("The file has no audio track")?;
    let track_id = track.id;
    let params = match &track.codec_params {
        Some(CodecParameters::Audio(p)) => p.clone(),
        _ => return Err("The file has no decodable audio track".to_string()),
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("Unsupported audio codec: {e}"))?;

    let mut resampler: Option<Resampler> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut out: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // Containers may signal end-of-stream as an EOF IO error.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("Reading media: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(SymphoniaError::DecodeError(_)) => continue, // skip a corrupt packet
            Err(e) => return Err(format!("Decoding audio: {e}")),
        };
        let channels = buf.spec().channels().count().max(1);
        let rate = buf.spec().rate();
        interleaved.clear();
        buf.copy_to_vec_interleaved(&mut interleaved);
        let mono = downmix(&interleaved, channels);
        let resampler = resampler.get_or_insert_with(|| Resampler::new(rate, TARGET_RATE));
        out.extend(resampler.process(&mono).into_iter().map(to_i16));
        if out.len() > MAX_DECODE_SECONDS * TARGET_RATE as usize {
            return Err(
                "Files longer than 2 hours aren't supported — split the file first.".into(),
            );
        }
    }
    if out.is_empty() {
        return Err("No audio could be decoded from the file".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("myjavis-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn downmix_averages_interleaved_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn wav_header_describes_the_samples() {
        let wav = encode_wav(&[0, 1, -1], 16_000, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            16_000
        );
    }

    #[test]
    fn decodes_48k_stereo_wav_to_16k_mono() {
        // 1 s of 48 kHz stereo: left = 0.5, right = -0.5 → mono ≈ 0.
        let frames = 48_000;
        let mut interleaved = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            interleaved.push(16_384i16);
            interleaved.push(-16_384i16);
        }
        let path = temp_file("in.wav");
        std::fs::write(&path, encode_wav(&interleaved, 48_000, 2)).unwrap();

        let pcm = decode_file_to_pcm16k(&path).unwrap();

        assert!((15_900..=16_100).contains(&pcm.len()), "len {}", pcm.len());
        assert!(pcm.iter().all(|s| s.abs() < 200), "expected near-silence");
    }

    #[test]
    fn rejects_files_that_are_not_media() {
        let path = temp_file("notes.txt");
        std::fs::write(&path, b"definitely not audio").unwrap();
        assert!(decode_file_to_pcm16k(&path).is_err());
    }
}

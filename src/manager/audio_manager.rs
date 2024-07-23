use std::{num::NonZeroU16, time::Duration};

use basedrop::{Collector, Handle, Shared};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{
    channel::Pan,
    live_audio::{AudioMsgConfig, FromWorkerMsg, LiveAudio, ToWorkerMsg},
    sample::{SampleData, SampleMetaData},
    song::song::{InternalSong, Song, SongOperation},
};

pub struct AudioManager {
    song: simple_left_right::writer::Writer<InternalSong, SongOperation>,
    gc: Collector,
    gc_handle: Handle,
    stream: Option<(cpal::Stream, std::sync::mpsc::Sender<ToWorkerMsg>)>,
}

impl AudioManager {
    pub fn new(song: Song) -> Self {
        let gc = basedrop::Collector::new();
        let gc_handle = gc.handle();
        let left_right = simple_left_right::writer::Writer::new(song.to_internal(&gc_handle));

        Self {
            song: left_right,
            gc,
            gc_handle,
            stream: None,
        }
    }

    pub fn get_devices() -> cpal::OutputDevices<cpal::Devices> {
        cpal::default_host().output_devices().unwrap()
    }

    pub fn default_device() -> Option<cpal::Device> {
        cpal::default_host().default_output_device()
    }

    /// may block
    pub fn edit_song(&mut self) -> SongEdit {
        SongEdit {
            song: std::mem::ManuallyDrop::new(self.song.lock()),
            gc_handle: &self.gc_handle,
        }
    }

    pub fn collect_garbage(&mut self) {
        self.gc.collect()
    }

    /// If the config specifies more than two channels only the first two will be filled with audio.
    /// The rest gets silence.
    /// audio_msg_config and msg_buffer_size allow you to configure the messages of the audio stream
    /// depending on your application. When the channel is full messages get dropped.
    /// currently panics when there is already a stream. needs better behaviour
    pub fn init_audio(
        &mut self,
        device: cpal::Device,
        config: OutputConfig,
        audio_msg_config: AudioMsgConfig,
        msg_buffer_size: usize,
    ) -> Result<futures::channel::mpsc::Receiver<FromWorkerMsg>, cpal::BuildStreamError> {
        let from_worker = futures::channel::mpsc::channel(msg_buffer_size);
        let to_worker = std::sync::mpsc::channel();
        let reader = self.song.build_reader().unwrap();

        let audio_worker =
            LiveAudio::new(reader, to_worker.1, audio_msg_config, from_worker.0, config);

        let stream = device.build_output_stream_raw(
            &config.into(),
            cpal::SampleFormat::F32,
            audio_worker.get_generic_callback(),
            |err| println!("{err}"),
            None,
        )?;

        stream.play().unwrap();

        self.stream = Some((stream, to_worker.0));

        Ok(from_worker.1)
    }

    /// need to think about the API more
    pub fn play_note(&self, note_event: crate::song::note_event::NoteEvent) {
        if let Some((_, channel)) = &self.stream {
            channel.send(ToWorkerMsg::PlayEvent(note_event)).unwrap();
        }
    }
}

/// the changes made to the song will be made available to the playing live audio as soon as
/// this struct is dropped.
/// With this you can load the full song without ever playing a half initialised state
/// when doing mulitple operations this object should be kept as it is
// should do all the verfication of
// need manuallyDrop because i need consume on drop behaviour
pub struct SongEdit<'a> {
    song: std::mem::ManuallyDrop<
        simple_left_right::writer::WriteGuard<'a, InternalSong, SongOperation>,
    >,
    gc_handle: &'a Handle,
}

impl SongEdit<'_> {
    pub fn set_sample(&mut self, num: usize, meta: SampleMetaData, data: SampleData) {
        assert!(num < Song::MAX_SAMPLES);
        let op = SongOperation::SetSample(num, meta, Shared::new(self.gc_handle, data));
        self.song.apply_op(op);
    }

    pub fn set_volume(&mut self, channel: usize, volume: u8) {
        assert!(channel < Song::MAX_CHANNELS);
        let op = SongOperation::SetVolume(channel, volume);
        self.song.apply_op(op);
    }

    pub fn set_pan(&mut self, channel: usize, pan: Pan) {
        assert!(channel < Song::MAX_CHANNELS);
        let op = SongOperation::SetPan(channel, pan);
        self.song.apply_op(op);
    }
}

impl Drop for SongEdit<'_> {
    fn drop(&mut self) {
        // SAFETY:
        // the ManuallyDrop isn't used after this as this is the drop function
        // can't use into_inner, as i only have &mut not owned
        unsafe { std::mem::ManuallyDrop::take(&mut self.song) }.swap()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OutputConfig {
    pub buffer_size: u32,
    pub channel_count: NonZeroU16,
    pub sample_rate: u32,
}

impl From<OutputConfig> for cpal::StreamConfig {
    fn from(value: OutputConfig) -> Self {
        cpal::StreamConfig {
            channels: value.channel_count.into(),
            sample_rate: cpal::SampleRate(value.sample_rate),
            buffer_size: cpal::BufferSize::Fixed(value.buffer_size),
        }
    }
}

impl TryFrom<cpal::StreamConfig> for OutputConfig {
    type Error = ();

    /// fails if BufferSize isn't explicit or zero output channels are specified.
    fn try_from(value: cpal::StreamConfig) -> Result<Self, Self::Error> {
        match value.buffer_size {
            cpal::BufferSize::Default => Err(()),
            cpal::BufferSize::Fixed(size) => Ok(OutputConfig {
                buffer_size: size,
                channel_count: NonZeroU16::try_from(value.channels).map_err(|_| ())?,
                sample_rate: value.sample_rate.0,
            }),
        }
    }
}
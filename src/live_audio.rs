use std::ops::{AddAssign, IndexMut};

use crate::audio_processing::playback::{PlaybackState, PlaybackStatus};
use crate::audio_processing::sample::Interpolation;
use crate::audio_processing::sample::SamplePlayer;
use crate::audio_processing::Frame;
use crate::manager::{OutputConfig, ToWorkerMsg};
use crate::project::song::Song;
use dasp::sample::ToSample;
use simple_left_right::Reader;

pub(crate) struct LiveAudio {
    song: Reader<Song<true>>,
    playback_state: Option<PlaybackState<'static, true>>,
    live_note: Option<SamplePlayer<'static, true>>,
    manager: rtrb::Consumer<ToWorkerMsg>,
    state_sender: triple_buffer::Input<Option<PlaybackStatus>>,
    config: OutputConfig,

    buffer: Box<[Frame]>,
}

const INTERPOLATION: u8 = Interpolation::Linear as u8;

impl LiveAudio {
    /// Not realtime safe.
    pub fn new(
        song: Reader<Song<true>>,
        manager: rtrb::Consumer<ToWorkerMsg>,
        state_sender: triple_buffer::Input<Option<PlaybackStatus>>,
        config: OutputConfig,
    ) -> Self {
        Self {
            song,
            playback_state: None,
            live_note: None,
            manager,
            state_sender,
            config,
            buffer: vec![Frame::default(); config.buffer_size.try_into().unwrap()].into(),
        }
    }

    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    fn send_state(&mut self) {
        self.state_sender
            .write(self.playback_state.as_ref().map(|s| s.get_status()));
    }

    // #[inline(never)]
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    /// returns true if work was done
    fn fill_internal_buffer(&mut self) -> bool {
        let song = self.song.lock();

        // process manager events
        while let Ok(event) = self.manager.pop() {
            match event {
                ToWorkerMsg::StopPlayback => self.playback_state = None,
                ToWorkerMsg::Playback(settings) => {
                    self.playback_state =
                        PlaybackState::<true>::new(&song, self.config.sample_rate, settings);
                }
                ToWorkerMsg::PlayEvent(note) => {
                    if let Some(sample) = &song.samples[usize::from(note.sample_instr)] {
                        let sample_player = SamplePlayer::new(
                            (sample.0, sample.1.get_handle()),
                            self.config.sample_rate / 2,
                            note.note,
                        );
                        self.live_note = Some(sample_player);
                    }
                }
                ToWorkerMsg::StopLiveNote => self.live_note = None,
            }
        }
        if self.live_note.is_none() && self.playback_state.is_none() {
            // no processing todo
            return false;
        }

        // clear buffer from past run
        // only happens if there is work todo
        self.buffer.fill(Frame::default());

        // process live_note
        if let Some(live_note) = &mut self.live_note {
            let note_iter = live_note.iter::<{ INTERPOLATION }>();
            self.buffer
                .iter_mut()
                .zip(note_iter)
                .for_each(|(buf, note)| buf.add_assign(note));

            if live_note.check_position().is_break() {
                self.live_note = None;
            }
        }

        // // process song playback
        if let Some(playback) = &mut self.playback_state {
            let playback_iter = playback.iter::<{ INTERPOLATION }>(&song);
            self.buffer
                .iter_mut()
                .zip(playback_iter)
                .for_each(|(buf, frame)| buf.add_assign(frame));

            if playback.is_done() {
                self.playback_state = None;
            }
        }

        true
    }

    /// converts the internal buffer to any possible output format and channel count
    /// sums stereo to mono and fills channels 3 and up with silence
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    #[inline]
    fn fill_from_internal<Sample: dasp::sample::Sample + dasp::sample::FromSample<f32>>(
        &mut self,
        data: &mut [Sample],
    ) {
        // convert the internal buffer and move it to the out_buffer
        if self.config.channel_count.get() == 1 {
            data.iter_mut()
                .zip(self.buffer.iter())
                .for_each(|(out, buf)| *out = buf.sum_to_mono().to_sample_());
        } else {
            data.chunks_exact_mut(usize::from(self.config.channel_count.get()))
                .map(|frame| frame.split_first_chunk_mut::<2>().unwrap().0)
                .zip(self.buffer.iter())
                .for_each(|(out, buf)| *out = buf.to_sample());
        }
    }

    // unsure wether i want to use this or untyped_callback
    // also relevant when cpal gets made into a generic that maybe this gets useful
    pub fn get_typed_callback<Sample: dasp::sample::Sample + dasp::sample::FromSample<f32>>(
        mut self,
    ) -> impl FnMut(&mut [Sample]) {
        move |data| {
            // assert_eq!(
            //     data.len(),
            //     usize::try_from(self.config.buffer_size).unwrap()
            //         * usize::from(self.config.channel_count.get())
            // );

            if self.fill_internal_buffer() {
                self.fill_from_internal(data);
            }
            self.send_state();
        }
        // move |data, info| {
        //     assert_eq!(
        //         data.len(),
        //         usize::try_from(self.config.buffer_size).unwrap()
        //             * usize::from(self.config.channel_count.get())
        //     );
        // self.send_state(Some(info));
        // }
    }

    // pub fn get_callback(mut self) -> impl FnMut(&mut [Frame], S::BufferInformation) {
    //     move |data, info| {
    //         assert_eq!(data.len(), self.config.buffer_size as usize * self.config.channel_count.get() as usize)

    //         if self.fill_internal_buffer() {
    //             self.fill_from_internal(data);
    //         }
    //     }
    // }
}

// only used for testing
// if not testing is unused
#[allow(dead_code)]
fn sine(output: &mut [[f32; 2]], sample_rate: f32) {
    let mut sample_clock = 0f32;
    for frame in output {
        sample_clock = (sample_clock + 1.) % sample_rate;
        let value = (sample_clock * 440. * 2. * std::f32::consts::PI / sample_rate).sin();
        *frame.index_mut(0) = value;
        *frame.index_mut(1) = value;
    }
}

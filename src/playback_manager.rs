use crate::constants::*;
use crate::interface::EffectMessage;
use crate::Sample;
use ringbuf::{Consumer, Producer};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub struct PlaybackManager {
    loops: Consumer<f32>,
    sample_counter: usize,      // counts samples in current measure
    samples_per_measure: usize, // only supports 4/4 for now
    stream: Producer<Sample>,
    effects_message_receiver: Arc<Mutex<Receiver<EffectMessage>>>,
}

impl PlaybackManager {
    pub fn new(
        loops: Consumer<f32>,
        stream: Producer<Sample>,
        samples_per_measure: usize,
        effects_message_receiver: Receiver<EffectMessage>,
    ) -> Self {
        Self {
            loops,
            stream,
            samples_per_measure,
            sample_counter: 0,
            effects_message_receiver: Arc::new(Mutex::new(effects_message_receiver)),
        }
    }

    fn send_stream(&mut self, stream: &[f32]) {
        for sample in stream.iter() {
            let remaining_samples_in_measure = self.samples_per_measure - self.sample_counter;
            if remaining_samples_in_measure == 0 {
                self.stream
                    .push(Sample::Tick)
                    .expect("stream ring buffer full");
                self.sample_counter = 0;
            } else if remaining_samples_in_measure == BUFFER_SIZE {
                self.stream
                    .push(Sample::PreTick)
                    .expect("stream ring buffer full");
            }
            self.stream
                .push(Sample::Data(*sample))
                .expect("stream ring buffer full");
            self.sample_counter += 1;
        }
    }

    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        let compress = false;
        if compress {
            let scale = Self::compressor(input);
            input
                .iter()
                .zip(output.iter_mut())
                .for_each(|(x, y)| *y = x * scale);
        } else {
            output.copy_from_slice(input);
        }

        let distort = false;
        if distort {
            let wet = Self::distortion(output);
            let mix = 0.10;
            for (x, wet) in output.iter_mut().zip(wet.iter()) {
                *x = *x * (1.0 - mix) + wet * mix;
            }
        }

        self.send_stream(output);
        self.play_loops(output);
    }

    fn play_loops(&mut self, output: &mut [f32]) {
        for sample in output.iter_mut() {
            *sample += self.loops.pop().expect("loop buffer empty")
        }
    }

    // compressor - basically a rust rewrite of Bart's compressor
    // might do a better algorithm that has a attack and release adjustment
    // because when the compressor shuts off below the threshold it
    // sounds pretty bad (pop/click noise). Also, I am using my Jack's buffer
    // frame size (usually 512 samples) which is pretty short.
    // I could save older samples with a ring buffer if I wanted a larger period
    fn compressor(buffer: &[f32]) -> f32 {
        // for calculating peak amplitude.
        let threshold = -50.0;
        let ratio = 4.0;

        let peak = Self::peak(buffer);

        if peak >= threshold {
            10.0_f32.powf(
                ((peak - threshold) * (1.0 / ratio - 1.0) + threshold * (1.0 / ratio - 1.0)) / 20.0,
            )
        } else {
            1.0
        }
    }

    fn distortion(buffer: &[f32]) -> [f32; BUFFER_SIZE] {
        // hopefully this gets optimized out?
        let table: Vec<f32> = (0..1000)
            .map(|x| (x as f32 * 3.0 / 1000.0).atan() * 0.8)
            .collect();

        let waveshape = |x: f32| {
            let x = (x * 1000.0).trunc() as i32;
            if x >= 0 && x < 1000 {
                table[x as usize]
            } else if x < 0 && x > -1000 {
                -1.0 * table[(-x) as usize]
            } else if x > 1000 {
                table[999]
            } else {
                -1.0 * table[999]
            }
        };

        let mut output = [0.0; BUFFER_SIZE];
        for (x, y) in buffer.iter().zip(output.iter_mut()) {
            *y = waveshape(*x);
        }

        output
    }

    fn peak(buffer: &[f32]) -> f32 {
        let (max, min) = buffer
            .iter()
            .fold((buffer[0], buffer[0]), |(max, min), &x| {
                if x > max {
                    (x, min)
                } else if x < min {
                    (max, x)
                } else {
                    (max, min)
                }
            });

        20.0 * (max - min).log10()
    }
}

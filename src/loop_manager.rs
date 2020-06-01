use crate::constants::*;
use crate::interface::LoopMessage;
use crate::Sample;
use hound::WavReader;
use ringbuf::{Consumer, Producer};
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoopStatus {
    Off,
    On(usize),
    RecordStart,
    Recording,
    RecordEnd,
    Empty,
}

impl LoopStatus {
    fn increment(&mut self, length: usize) {
        match self {
            LoopStatus::On(x) => {
                *x += 1;
                *x %= length;
            }
            _ => panic!("tried to increment inactive loop"),
        }
    }
}

pub struct LoopManager {
    loops: Vec<Vec<f32>>,
    lengths: Vec<usize>,
    status: Vec<LoopStatus>,
    playback: Producer<f32>,
    stream: Consumer<Sample>,
    samples_per_measure: usize,
    loop_message_receiver: Receiver<LoopMessage>,
    any_recording: bool,
    recording_at_index: usize,
}

impl LoopManager {
    pub fn new(
        mut playback: Producer<f32>,
        stream: Consumer<Sample>,
        samples_per_beat: usize,
        loop_message_receiver: Receiver<LoopMessage>,
    ) -> Self {
        let mut wav = WavReader::open("metronome/big_tick.wav").unwrap();
        let mut big_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();
        wav = WavReader::open("metronome/little_tick.wav").unwrap();
        let little_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();
        let silence = samples_per_beat - little_tick.len();
        let samples_per_measure = samples_per_beat * 4;

        let mut metronome = vec![];
        metronome.append(&mut big_tick);
        metronome.append(
            &mut (0..samples_per_beat)
                .skip(metronome.len())
                .map(|_| 0.0)
                .collect(),
        );

        for _ in 0..3 {
            for &sample in little_tick.iter() {
                metronome.push(sample);
            }
            for _ in 0..silence {
                metronome.push(0.0);
            }
        }
        assert_eq!(metronome.len(), samples_per_measure);

        let mut loops = vec![vec![]; NUM_LOOPS];
        let mut status = vec![LoopStatus::Empty; NUM_LOOPS];
        let mut lengths = vec![0; NUM_LOOPS];

        loops[0].append(&mut metronome);
        lengths[0] = 1;
        status[0] = LoopStatus::On(0);
        for i in 0..samples_per_measure {
            playback.push(loops[0][i]).unwrap();
        }

        Self {
            samples_per_measure,
            loops,
            status,
            lengths,
            stream,
            playback,
            loop_message_receiver,
            any_recording: false,
            recording_at_index: 0,
        }
    }

    pub fn run(mut self) {
        std::thread::spawn(move || loop {
            if let Some(sample) = self.stream.pop() {
                match sample {
                    Sample::PreTick => self.enqueue_loops(),
                    Sample::Tick => self.update_recording_status(),
                    Sample::Data(sample) => {
                        if self.any_recording {
                            self.loops[self.recording_at_index].push(sample)
                        }
                    }
                }
            }
            self.check_messages();
        });
    }

    fn enqueue_loops(&mut self) {
        // If a recording is pending finishing
        let mut partial_recording = false;
        // (offset, loop_data index) for each active loop
        let active_loops: Vec<(usize, usize)> = (0..self.loops.len())
            .zip(self.status.iter())
            .filter_map(|(loop_data, status)| {
                let active;
                let offset;
                match status {
                    LoopStatus::On(x) => {
                        active = true;
                        offset = *x;
                    }
                    LoopStatus::RecordEnd => {
                        active = true;
                        offset = 0;
                        partial_recording = true;
                    }
                    _ => {
                        active = false;
                        offset = 0;
                    }
                };
                if !active {
                    None
                } else {
                    Some((offset, loop_data))
                }
            })
            .collect();

        for i in 0..self.samples_per_measure {
            let mixed_sample = active_loops.iter().fold(0.0, |acc, (offset, loop_index)| {
                acc + self.loops[*loop_index][offset * self.samples_per_measure + i]
            });
            self.playback
                .push(mixed_sample)
                .expect("playback buffer full");
            // if still actively recording next loop then
            // stop after writing half of the samples to the ringbuf
            // to finish recording
            if partial_recording && i == self.samples_per_measure / 2 {
                self.finish_recording_loop();
            }
        }
        for (_, loop_index) in active_loops.iter() {
            self.status[*loop_index].increment(self.lengths[*loop_index]);
        }
    }

    fn finish_recording_loop(&mut self) {
        let index = self
            .status
            .iter()
            .enumerate()
            .find(|(_, status)| **status == LoopStatus::RecordEnd)
            .expect("no ending recording found in finish recording")
            .0;

        self.lengths[index] += 1;
        self.status[index] = LoopStatus::On(0);
        //self.status[index].increment(self.lengths[index]);
        self.any_recording = false;

        loop {
            if let Some(sample) = self.stream.pop() {
                match sample {
                    Sample::Data(x) => self.loops[index].push(x),
                    Sample::Tick => {
                        assert_eq!(0, self.loops[index].len() % self.samples_per_measure);
                        return;
                    }
                    Sample::PreTick => panic!("things are very broken"),
                }
            }
        }
    }

    fn update_recording_status(&mut self) {
        for (i, status) in self.status.iter_mut().enumerate() {
            if *status == LoopStatus::RecordEnd {
                panic!("recording should be finished by other func");
            }

            if *status == LoopStatus::RecordStart {
                *status = LoopStatus::Recording;
                self.any_recording = true;
                self.recording_at_index = i;
                return;
            }

            if *status == LoopStatus::Recording {
                self.lengths[i] += 1;
                self.any_recording = true;
                self.recording_at_index = i;
                return;
            }
        }
    }

    fn check_messages(&mut self) {
        if let Ok(message) = self.loop_message_receiver.try_recv() {
            match message {
                LoopMessage::ToggleLoop(index) => match self.status[index] {
                    LoopStatus::Off => self.status[index] = LoopStatus::On(0),
                    LoopStatus::On(_) => self.status[index] = LoopStatus::Off,
                    LoopStatus::Empty => self.status[index] = LoopStatus::RecordStart,
                    _ => (),
                },
                LoopMessage::StopRecording => self.stop_recording(),
            }
        }
    }

    fn stop_recording(&mut self) {
        self.status = self
            .status
            .iter()
            .map(|status| match status {
                LoopStatus::Recording => LoopStatus::RecordEnd,
                _ => *status,
            })
            .collect();
    }
}

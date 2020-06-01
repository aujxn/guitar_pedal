use crate::constants::*;
use crate::interface::LoopMessage;
use crate::Sample;
use hound::WavReader;
use ringbuf::{Consumer, Producer};
use std::sync::mpsc::Receiver;

/// Tracks the status of a single loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoopStatus {
    Off,
    /// The data here keeps track of which measure the loop is in,
    /// since loops can be multiple measures in length.
    On(usize),
    /// Loop is going to start recording next measure.
    RecordStart,
    Recording,
    /// Loop is going to stop recording next measure.
    RecordEnd,
    Empty,
}

impl LoopStatus {
    /// Helper method for the LoopManager to move a loop into its next measure.
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

/// Records, stores, activates, deactivates, and mixes loops.
pub struct LoopManager {
    /// The loops data in Vecs of samples
    loops: Vec<Vec<f32>>,
    /// The length in measures of each loop
    lengths: Vec<usize>,
    /// The status of each loop.
    status: Vec<LoopStatus>,
    /// RingBuffer for sending mixed loops to the PlaybackManager
    playback: Producer<f32>,
    /// RingBuffer stream of samples with clock info from PlaybackManager
    stream: Consumer<Sample>,
    samples_per_measure: usize,
    /// Message receiver from Interface for instructions
    loop_message_receiver: Receiver<LoopMessage>,
    /// Is a loop currently being recorded?
    any_recording: bool,
    /// What index are we currently recording at?
    recording_at_index: usize,
}

impl LoopManager {
    pub fn new(
        mut playback: Producer<f32>,
        stream: Consumer<Sample>,
        samples_per_beat: usize,
        loop_message_receiver: Receiver<LoopMessage>,
    ) -> Self {
        // Loop index 0 will be a metronome with a loud tick and 3 soft ticks
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
        // Sanity check that math is real
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

    /// Non-blocking function to activate LoopManager.
    pub fn run(mut self) {
        std::thread::spawn(move || loop {
            // try to get a sample from the PlaybackManager
            if let Some(sample) = self.stream.pop() {
                match sample {
                    // Mix the loops and push them to the RingBuffer
                    Sample::PreTick => self.enqueue_loops(),
                    // Start recording if there are any loops pending start,
                    // if currently recording then update the length
                    Sample::Tick => self.update_recording_status(),
                    // A real sample! If we are recording then save it
                    Sample::Data(sample) => {
                        if self.any_recording {
                            self.loops[self.recording_at_index].push(sample)
                        }
                    }
                }
            }
            // Check to see if there are any messages from the Interface
            self.check_messages();
        });
    }

    /// Mixes all the active (LoopStatus::On or LoopStatus::RecordEnd) loops
    /// and push the mixed audio into the RingBuffer for the PlaybackManager.
    fn enqueue_loops(&mut self) {
        // If a recording is pending finishing then we want to start playing
        // it next measure. But we don't have all of the data for that loop
        // yet... So we will deal with that later.
        let partial_recording = self.status[self.recording_at_index] == LoopStatus::RecordEnd;

        // (offset, loop_data index) for each active loop where offset is the
        // measure that the loop is in. Collecting this information into a Vec
        // isn't the most efficient way to do this but it has no issue completing
        // in the 3-5ms it has before the PlaybackManager needs samples.
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

        // Mix the loops that we found --- push data one at a time because the
        // PlaybackManager wants this info soon so send it as we compute it
        // instead of computing all and then pushing the whole thing to the buffer.
        for i in 0..self.samples_per_measure {
            // Mix a single sample. Don't worry about clippin management because the
            // Jack server will do that for us. TODO: output a message if the mixed
            // loops start clipping so the user can know.
            let mixed_sample = active_loops.iter().fold(0.0, |acc, (offset, loop_index)| {
                acc + self.loops[*loop_index][offset * self.samples_per_measure + i]
            });
            self.playback
                .push(mixed_sample)
                .expect("playback buffer full");

            // Remember that we might still be recording a loop that we would
            // like to play this measure? Well here is the problem: We dont have
            // that entire loop recorded because we started our mixing computation
            // a buffer frame early to make sure we stay ahead of the playback.
            // So after we compute the mix for half of the loop samples let us
            // finish recording the almost complete loop. There should only be
            // one buffer frame left to do (512 sample for my jack server settings)
            // so this should give plenty wiggle room in both directions.
            if partial_recording && i == self.samples_per_measure / 2 {
                self.finish_recording_loop();
            }
        }
        // Increment each active loop so the correct measure plays next time
        for (_, loop_index) in active_loops.iter() {
            self.status[*loop_index].increment(self.lengths[*loop_index]);
        }
    }

    /// Gets the rest of the pending completion loop. See large comment in
    /// fn enqueue_loops.
    fn finish_recording_loop(&mut self) {
        let index = self.recording_at_index;
        // Crash the program if the currently recording loop isn't pending finish.
        assert_eq!(self.status[index], LoopStatus::RecordEnd);

        self.lengths[index] += 1;
        self.status[index] = LoopStatus::On(0);
        self.any_recording = false;

        loop {
            if let Some(sample) = self.stream.pop() {
                match sample {
                    Sample::Data(x) => self.loops[index].push(x),
                    Sample::Tick => {
                        // Done! Sanity check the length and get back to mixing.
                        assert_eq!(0, self.loops[index].len() % self.samples_per_measure);
                        return;
                    }
                    // This function should be initiated quickly after a PreTick
                    // so another PreTick means there are issues.
                    Sample::PreTick => panic!("things are very broken"),
                }
            }
        }
    }

    /// Increase length of loops that are recording and start recording if any
    /// loops are pending start.
    fn update_recording_status(&mut self) {
        // If recording update the length.
        if self.any_recording {
            self.lengths[self.recording_at_index] += 1;
        } else {
            for (i, status) in self.status.iter_mut().enumerate() {
                // A pending finish recording should always be cleaned up by
                // enqueue_loops which calls finish_recording_loop.
                if *status == LoopStatus::RecordEnd {
                    panic!("recording should be finished by other func");
                }

                // If pending start then set state to begin saving samples.
                if *status == LoopStatus::RecordStart {
                    *status = LoopStatus::Recording;
                    self.any_recording = true;
                    self.recording_at_index = i;
                    return;
                }
            }
        }
    }

    /// Checks receiver from Interface message channel.
    fn check_messages(&mut self) {
        if let Ok(message) = self.loop_message_receiver.try_recv() {
            match message {
                LoopMessage::ToggleLoop(index) => match self.status[index] {
                    LoopStatus::Off => self.status[index] = LoopStatus::On(0),
                    // This essentially stops the loop at the start of the next
                    // measure. Maybe the loop should play out to completion
                    // if it is multiple measures is length? TODO
                    LoopStatus::On(_) => self.status[index] = LoopStatus::Off,
                    LoopStatus::Empty => {
                        if self.any_recording {
                            println!("Already recording at index {}", self.recording_at_index);
                        } else {
                            self.status[index] = LoopStatus::RecordStart;
                        }
                    }
                    LoopStatus::Recording => println!("Currently recording here already"),
                    LoopStatus::RecordStart => {
                        println!("Already going to record here starting next measure")
                    }
                    LoopStatus::RecordEnd => println!("Wrapping up a recording here already"),
                },
                LoopMessage::StopRecording => self.stop_recording(),
            }
        }
    }

    /// Sets a currently recording loop's status to pending completion.
    fn stop_recording(&mut self) {
        if !self.any_recording {
            println!("Not currently recording anything!");
        } else {
            self.status[self.recording_at_index] = LoopStatus::RecordEnd;
        }
    }
}

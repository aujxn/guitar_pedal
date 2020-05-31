use guitar_pedal::midi::MidiEvent;
use hound::WavReader;
use jack;
use ringbuf::{Consumer, Producer, RingBuffer};
use std::io;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use structopt::StructOpt;

const BUFFER_SIZE: usize = 512;
const SAMPLE_RATE: usize = 48000;
const SAMPLES_PER_MINUTE: usize = SAMPLE_RATE * 60;
const NUM_LOOPS: usize = 24;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(short, long, default_value = "80")]
    bpm: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LoopStatus {
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
            LoopStatus::On(x) => *x = (*x + 1) % length,
            _ => panic!("tried to increment inactive loop"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Sample {
    PreTick,
    Tick,
    Data(f32),
}

enum LoopMessage {
    // Starts recording loop if none exists, otherwise toggles on/off
    ToggleLoop(usize),
    StopRecording,
}

enum EffectMessage {
    ToggleCompression,
    ToggleDistortion,
}

struct Interface {
    loop_message_sender: SyncSender<LoopMessage>,
    effects_message_sender: SyncSender<EffectMessage>,
    midi_rx: Receiver<MidiEvent>,
}

impl Interface {
    fn new(
        loop_message_sender: SyncSender<LoopMessage>,
        effects_message_sender: SyncSender<EffectMessage>,
    ) -> Self {
        let midi_rx = guitar_pedal::midi::listen_for_midi();

        Interface {
            loop_message_sender,
            effects_message_sender,
            midi_rx,
        }
    }
    fn run(self) {
        std::thread::spawn(move || loop {
            let midi_event = self
                .midi_rx
                .recv()
                .expect("midi_event reciever channel error");
            if midi_event.data[0] == 144 && midi_event.data[1] < 60 {
                println!("{:?}", midi_event);
                let _ = self
                    .loop_message_sender
                    .send(LoopMessage::ToggleLoop(midi_event.data[1] as usize - 36))
                    .unwrap();
            } else if midi_event.data[0] == 176
                && midi_event.data[1] == 64
                && midi_event.data[2] <= 63
            {
                println!("Stopping recording");
                let _ = self
                    .loop_message_sender
                    .send(LoopMessage::StopRecording)
                    .unwrap();
            }
        });
    }
}

struct LoopManager {
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
    fn run(mut self) {
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

        //self.lengths[index] += 1;
        self.status[index] = LoopStatus::On(0);
        self.status[index].increment(self.lengths[index]);
        self.any_recording = false;

        loop {
            match self
                .stream
                .pop()
                .expect("stream is empty in finish recording")
            {
                Sample::Data(x) => self.loops[index].push(x),
                Sample::Tick => {
                    assert_eq!(0, self.loops[index].len() % self.samples_per_measure);
                    return;
                }
                Sample::PreTick => panic!("things are very broken"),
            }
        }
    }

    fn update_recording_status(&mut self) {
        self.status = self
            .status
            .iter()
            .map(|status| match status {
                LoopStatus::RecordEnd => panic!("recording should be finished by other func"),
                LoopStatus::RecordStart => LoopStatus::Recording,
                _ => *status,
            })
            .collect();

        for (i, status) in self.status.iter().enumerate() {
            if *status == LoopStatus::Recording {
                self.lengths[i] += 1;
                self.any_recording = true;
                self.recording_at_index = i;
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

struct PlaybackManager {
    loops: Consumer<f32>,
    sample_counter: usize,      // counts samples in current measure
    samples_per_measure: usize, // only supports 4/4 for now
    stream: Producer<Sample>,
    effects_message_receiver: Arc<Mutex<Receiver<EffectMessage>>>,
}

fn init(bpm: usize) -> (PlaybackManager, LoopManager, Interface) {
    let mut wav = WavReader::open("big_tick.wav").unwrap();
    let mut big_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();
    wav = WavReader::open("little_tick.wav").unwrap();
    let little_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();

    let samples_per_beat = SAMPLES_PER_MINUTE / bpm;
    let samples_per_measure = samples_per_beat * 4;
    let silence = samples_per_beat - little_tick.len();

    let (stream_producer, stream_consumer) = RingBuffer::new(samples_per_measure * 2).split();
    let (mut loop_producer, loop_consumer) = RingBuffer::new(samples_per_measure * 2).split();
    let (effects_message_sender, effects_message_receiver) = sync_channel(5);
    let (loop_message_sender, loop_message_receiver) = sync_channel(5);

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
        loop_producer.push(loops[0][i]).unwrap();
    }

    (
        PlaybackManager {
            loops: loop_consumer,
            sample_counter: 0,
            stream: stream_producer,
            samples_per_measure,
            effects_message_receiver: Arc::new(Mutex::new(effects_message_receiver)),
        },
        LoopManager {
            samples_per_measure,
            loops,
            status,
            lengths,
            stream: stream_consumer,
            playback: loop_producer,
            loop_message_receiver,
            any_recording: false,
            recording_at_index: 0,
        },
        Interface::new(loop_message_sender, effects_message_sender),
    )
}

impl PlaybackManager {
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

    fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
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
                -1.0 * table[(x * -1) as usize]
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

// Most of the code here was adapted from the playback_capture example from rust jack crate
fn main() {
    let opt = Opt::from_args();
    let (mut playback_manager, mut loop_manager, mut interface) = init(opt.bpm);
    loop_manager.run();
    interface.run();

    let (client, _status) =
        jack::Client::new("rust_jack_simple", jack::ClientOptions::NO_START_SERVER).unwrap();

    let in_b = client
        .register_port("rust_in_r", jack::AudioIn::default())
        .unwrap();
    let mut out_b = client
        .register_port("rust_out_r", jack::AudioOut::default())
        .unwrap();

    let process_callback = move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
        let output = out_b.as_mut_slice(ps);
        let input = in_b.as_slice(ps);
        playback_manager.process_block(input, output);
        jack::Control::Continue
    };

    let process = jack::ClosureProcessHandler::new(process_callback);

    // Activate the client, which starts the processing.
    let active_client = client.activate_async(Notifications, process).unwrap();

    // Wait for user input to quit
    println!("Press enter/return to quit...");
    let mut user_input = String::new();
    io::stdin().read_line(&mut user_input).ok();

    active_client.deactivate().unwrap();
}

// Notification Handler taken from playback_capture example in rust jack crate
struct Notifications;

impl jack::NotificationHandler for Notifications {
    fn thread_init(&self, _: &jack::Client) {
        println!("JACK: thread init");
    }

    fn shutdown(&mut self, status: jack::ClientStatus, reason: &str) {
        println!(
            "JACK: shutdown with status {:?} because \"{}\"",
            status, reason
        );
    }

    fn freewheel(&mut self, _: &jack::Client, is_enabled: bool) {
        println!(
            "JACK: freewheel mode is {}",
            if is_enabled { "on" } else { "off" }
        );
    }

    fn buffer_size(&mut self, _: &jack::Client, sz: jack::Frames) -> jack::Control {
        println!("JACK: buffer size changed to {}", sz);
        jack::Control::Continue
    }

    fn sample_rate(&mut self, _: &jack::Client, srate: jack::Frames) -> jack::Control {
        println!("JACK: sample rate changed to {}", srate);
        jack::Control::Continue
    }

    fn client_registration(&mut self, _: &jack::Client, name: &str, is_reg: bool) {
        println!(
            "JACK: {} client with name \"{}\"",
            if is_reg { "registered" } else { "unregistered" },
            name
        );
    }

    fn port_registration(&mut self, _: &jack::Client, port_id: jack::PortId, is_reg: bool) {
        println!(
            "JACK: {} port with id {}",
            if is_reg { "registered" } else { "unregistered" },
            port_id
        );
    }

    fn port_rename(
        &mut self,
        _: &jack::Client,
        port_id: jack::PortId,
        old_name: &str,
        new_name: &str,
    ) -> jack::Control {
        println!(
            "JACK: port with id {} renamed from {} to {}",
            port_id, old_name, new_name
        );
        jack::Control::Continue
    }

    fn ports_connected(
        &mut self,
        _: &jack::Client,
        port_id_a: jack::PortId,
        port_id_b: jack::PortId,
        are_connected: bool,
    ) {
        println!(
            "JACK: ports with id {} and {} are {}",
            port_id_a,
            port_id_b,
            if are_connected {
                "connected"
            } else {
                "disconnected"
            }
        );
    }

    fn graph_reorder(&mut self, _: &jack::Client) -> jack::Control {
        println!("JACK: graph reordered");
        jack::Control::Continue
    }

    fn xrun(&mut self, _: &jack::Client) -> jack::Control {
        println!("JACK: xrun occurred");
        jack::Control::Continue
    }

    fn latency(&mut self, _: &jack::Client, mode: jack::LatencyType) {
        println!(
            "JACK: {} latency has changed",
            match mode {
                jack::LatencyType::Capture => "capture",
                jack::LatencyType::Playback => "playback",
            }
        );
    }
}

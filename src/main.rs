use hound::WavReader;
use jack;
use std::io;
use structopt::StructOpt;

const BUFFER_SIZE: usize = 512;
const SAMPLE_RATE: usize = 48000;
const SAMPLES_PER_MINUTE: usize = SAMPLE_RATE * 60;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(short, long, default_value = "80")]
    bpm: usize,
}

struct PlaybackManager {
    loops: Vec<Vec<f32>>,
    loop_index: usize,
    samples_per_measure: usize, // only supports 4/4 for now
}

impl PlaybackManager {
    fn new(bpm: usize) -> Self {
        let mut wav = WavReader::open("big_tick.wav").unwrap();
        let mut big_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();
        wav = WavReader::open("little_tick.wav").unwrap();
        let little_tick: Vec<f32> = wav.samples().map(|x: Result<f32, _>| x.unwrap()).collect();

        let samples_per_beat = SAMPLES_PER_MINUTE / bpm;
        let samples_per_measure = samples_per_beat * 4;
        let silence = samples_per_beat - little_tick.len();

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
            // creating a loop with mostly 0's is really bad but is
            // simplest solution for now
            for _ in 0..silence {
                metronome.push(0.0);
            }
        }
        assert_eq!(metronome.len(), samples_per_measure);

        let loops = vec![metronome];
        let loop_index = 0;
        Self {
            loops,
            loop_index,
            samples_per_measure,
        }
    }

    fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        let compress = true;
        if compress {
            let scale = Self::compressor(input);
            input
                .iter()
                .zip(output.iter_mut())
                .for_each(|(x, y)| *y = x * scale);
        } else {
            output.copy_from_slice(input);
        }

        let wet = Self::distortion(output);
        let mix = 0.10;
        for (x, wet) in output.iter_mut().zip(wet.iter()) {
            *x = *x * (1.0 - mix) + wet * mix;
        }

        self.play_loops(output);
    }

    fn play_loops(&mut self, output: &mut [f32]) {
        for i in 0..BUFFER_SIZE {
            output[i] += self.loops.iter().fold(0.0, |acc, x| {
                acc + x[(i + self.loop_index) % self.samples_per_measure]
            });
        }

        self.loop_index += BUFFER_SIZE;
        self.loop_index %= self.samples_per_measure;
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
    let mut playback_manager = PlaybackManager::new(opt.bpm);

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

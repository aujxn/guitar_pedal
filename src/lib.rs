use constants::*;
use interface::Interface;
use loop_manager::LoopManager;
use notification_handler::Notifications;
use playback_manager::PlaybackManager;
use ringbuf::RingBuffer;
use std::sync::mpsc::sync_channel;

pub mod interface;
pub mod loop_manager;
/// Adapted from midi example in jack crate
pub mod midi;
/// Taken from playback_capture example in jack crate
pub mod notification_handler;
pub mod playback_manager;

/// Some global configuation constants
pub mod constants {
    /// The size of the JACK buffer frame
    pub const BUFFER_SIZE: usize = 512;
    pub const SAMPLE_RATE: usize = 48000;
    pub const SAMPLES_PER_MINUTE: usize = SAMPLE_RATE * 60;
    /// How many loop registers we want
    pub const NUM_LOOPS: usize = 24;
    /// What key will be loop ID of 0 (metronome)
    pub const LOOP_BASE_KEY: u8 = 36; //C2
    /// Which midi keys for distortion and compression
    pub const DISTORTION_KEY: u8 = 95; //B6
    pub const COMPRESSION_KEY: u8 = 96; //C7
    pub const MIDI_NOTE_DOWN: u8 = 144;
}

/// Used to stream the input from the PlaybackManager to the LoopManager.
#[derive(Clone, Copy, Debug)]
pub enum Sample {
    /// A single audio sample.
    Data(f32),
    /// A clock "Tick" to keep everything in sync just in case there are
    /// xruns or other inconsistency issues. PlaybackManager sends a Tick
    /// to the LoopManager at the start of each 4 beat measure.
    Tick,
    /// In order to avoid extra buffers and minimize latency, the PlaybackManager
    /// sends a clock "PreTick" in the last buffer frame of the measure. This
    /// gives the LoopManager time to figure out what loops are currently active
    /// and mix them together and send them back to the PlaybackManager before
    /// the next measure begins. This solution is kind of janky but it seems to work.
    PreTick,
}

/// Creates the PlaybackManager, LoopManager, and Interface.
pub fn init(bpm: usize) -> (PlaybackManager, LoopManager, Interface) {
    let samples_per_beat = SAMPLES_PER_MINUTE / bpm;
    // Only supports 4/4 signiture
    let samples_per_measure = samples_per_beat * 4;

    // Create ringbuffers for sending samples back and forth between
    // the LoopManager and PlaybackManager.
    let (stream_producer, stream_consumer) =
        RingBuffer::<Sample>::new(samples_per_measure * 2).split();
    let (loop_producer, loop_consumer) = RingBuffer::new(samples_per_measure * 2).split();

    // Create some channels to send messages from the Interface
    let (effects_message_sender, effects_message_receiver) = sync_channel(5);
    let (loop_message_sender, loop_message_receiver) = sync_channel(5);

    (
        PlaybackManager::new(
            loop_consumer,
            stream_producer,
            samples_per_measure,
            effects_message_receiver,
        ),
        LoopManager::new(
            loop_producer,
            stream_consumer,
            samples_per_beat,
            loop_message_receiver,
        ),
        Interface::new(loop_message_sender, effects_message_sender),
    )
}

/// Starts the Jack Client. Ports must be connected using a Jack Server tool like
/// Cadence, QjackCTL, or CLI tools. The Rust Jack connect ports utility can only
/// connect ports owned by clients it creates.
/// This function is non-blocking.
pub fn activate_client(mut playback_manager: PlaybackManager) {
    let (client, _status) =
        jack::Client::new("guitar_pedal", jack::ClientOptions::NO_START_SERVER).unwrap();

    std::thread::spawn(move || {
        let in_b = client
            .register_port("guitar_in", jack::AudioIn::default())
            .unwrap();
        let mut out_b = client
            .register_port("output", jack::AudioOut::default())
            .unwrap();

        let process_callback = move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            let output = out_b.as_mut_slice(ps);
            let input = in_b.as_slice(ps);
            playback_manager.process_block(input, output);
            jack::Control::Continue
        };

        let process = jack::ClosureProcessHandler::new(process_callback);

        let _active_client = client.activate_async(Notifications, process).unwrap();

        // client.activate_async is non-blocking and if this thread terminates the
        // client gets dropped. This thread is done working so just park it until
        // the program is done. I tried returning the client handle but rustc
        // was fighting me on how it was Sync so I just did this.
        loop {
            std::thread::park();
        }
    });
}

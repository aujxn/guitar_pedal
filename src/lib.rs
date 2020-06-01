use constants::*;
use interface::Interface;
use loop_manager::LoopManager;
use notification_handler::Notifications;
use playback_manager::PlaybackManager;
use ringbuf::RingBuffer;
use std::sync::mpsc::sync_channel;

pub mod interface;
pub mod loop_manager;
pub mod midi;
pub mod notification_handler;
pub mod playback_manager;

pub mod constants {
    pub const BUFFER_SIZE: usize = 512;
    pub const SAMPLE_RATE: usize = 48000;
    pub const SAMPLES_PER_MINUTE: usize = SAMPLE_RATE * 60;
    pub const NUM_LOOPS: usize = 24;
}

#[derive(Clone, Copy, Debug)]
pub enum Sample {
    PreTick,
    Tick,
    Data(f32),
}

pub fn init(bpm: usize) -> (PlaybackManager, LoopManager, Interface) {
    let samples_per_beat = SAMPLES_PER_MINUTE / bpm;
    let samples_per_measure = samples_per_beat * 4;

    let (stream_producer, stream_consumer) =
        RingBuffer::<Sample>::new(samples_per_measure * 2).split();
    let (loop_producer, loop_consumer) = RingBuffer::new(samples_per_measure * 2).split();
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

pub fn activate_client(mut playback_manager: PlaybackManager) {
    let (client, _status) =
        jack::Client::new("guitar_pedal", jack::ClientOptions::NO_START_SERVER).unwrap();

    std::thread::spawn(move || {
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

        let _active_client = client.activate_async(Notifications, process).unwrap();

        loop {
            std::thread::park();
        }
    });
}

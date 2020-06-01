use crate::constants::*;
use crate::midi::MidiEvent;
use std::sync::mpsc::{Receiver, SyncSender};

pub enum LoopMessage {
    // Starts recording loop if none exists, otherwise toggles on/off
    ToggleLoop(usize),
    StopRecording,
}

pub enum EffectMessage {
    ToggleCompression,
    ToggleDistortion,
}

pub struct Interface {
    loop_message_sender: SyncSender<LoopMessage>,
    effects_message_sender: SyncSender<EffectMessage>,
    midi_rx: Receiver<MidiEvent>,
}

impl Interface {
    pub fn new(
        loop_message_sender: SyncSender<LoopMessage>,
        effects_message_sender: SyncSender<EffectMessage>,
    ) -> Self {
        let midi_rx = crate::midi::listen_for_midi();

        Self {
            loop_message_sender,
            effects_message_sender,
            midi_rx,
        }
    }

    pub fn run(self) {
        std::thread::spawn(move || loop {
            let midi_event = self
                .midi_rx
                .recv()
                .expect("midi_event reciever channel error");
            if midi_event.data[0] == 144 && midi_event.data[1] < 36 + NUM_LOOPS as u8 {
                println!("{:?}", midi_event);
                self.loop_message_sender
                    .send(LoopMessage::ToggleLoop(midi_event.data[1] as usize - 36))
                    .unwrap();
            } else if midi_event.data[0] == 176
                && midi_event.data[1] == 64
                && midi_event.data[2] <= 63
            {
                println!("Stopping recording");
                self.loop_message_sender
                    .send(LoopMessage::StopRecording)
                    .unwrap();
            }
        });
    }
}

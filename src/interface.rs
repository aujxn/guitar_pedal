use crate::constants::*;
use crate::midi::MidiEvent;
use std::sync::mpsc::{Receiver, SyncSender};

/// Notification passed from the interface to the LoopManager.
pub enum LoopMessage {
    /// ToggleLoop sends and ID (index of the loop_manager.loops Vec)
    /// and turns that loop on if it is off, turns it loop off if it is
    /// on, and if that loop is empty it begins recording a loop for that
    /// slot starting on the next measure.
    ToggleLoop(usize),
    /// Stops any recording loop at the end of the current measure which
    /// this message was recieved.
    StopRecording,
}

/// Notification passed from the interface to the PlaybackManager.
// TODO: Maybe give the interface a way to change the settings of
// these effects
pub enum EffectMessage {
    /// Turns on and off compression.
    ToggleCompression,
    /// Turns on and off distortion.
    ToggleDistortion,
}

/// Receives MidiEvents from the midi module (src/midi.rs), interprets
/// them, and sends messages to the LoopManager and PlaybackManager.
pub struct Interface {
    /// Channel for sending messages to LoopManager.
    loop_message_sender: SyncSender<LoopMessage>,
    /// Channel for sending messages to PlaybackManager.
    effects_message_sender: SyncSender<EffectMessage>,
    /// Channel for receiving messages from midi module.
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

    /// Non-blocking function to start the interface.
    pub fn run(self) {
        std::thread::spawn(move || loop {
            // Block until MidiEvent arrives
            let midi_event = self
                .midi_rx
                .recv()
                .expect("midi_event reciever channel error");

            // 144 in the first data byte is note down press. My keyboard goes down to
            // C2 which is 36 in the second data byte. So make sure the key pressed
            // is in the range of how many loop we have and send a toggle message.
            if midi_event.data[0] == 144 && midi_event.data[1] < 36 + NUM_LOOPS as u8 {
                println!("{:?}", midi_event);
                self.loop_message_sender
                    .send(LoopMessage::ToggleLoop(midi_event.data[1] as usize - 36))
                    .unwrap();
            // [176, 64, <=63] is the down press of the sustain pedal. This is
            // ergonomically nicer for stopping recording than pressing a key
            // and because a recording should be happening already the
            // LoopManager doesn't need any extra information.
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

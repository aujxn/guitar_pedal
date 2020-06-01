use std::convert::From;
use std::sync::mpsc::sync_channel;

const MAX_MIDI: usize = 3;

//a fixed size container to copy data out of real-time thread
#[derive(Copy, Clone)]
pub struct MidiEvent {
    pub len: usize,
    pub data: [u8; MAX_MIDI],
    pub time: jack::Frames,
}

impl From<jack::RawMidi<'_>> for MidiEvent {
    fn from(midi: jack::RawMidi<'_>) -> Self {
        let len = std::cmp::min(MAX_MIDI, midi.bytes.len());
        let mut data = [0; MAX_MIDI];
        data[..len].copy_from_slice(&midi.bytes[..len]);
        MidiEvent {
            len,
            data,
            time: midi.time,
        }
    }
}

impl std::fmt::Debug for MidiEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Midi {{ time: {}, len: {}, data: {:?} }}",
            self.time,
            self.len,
            &self.data[..self.len]
        )
    }
}

pub fn listen_for_midi() -> std::sync::mpsc::Receiver<MidiEvent> {
    let (client, _status) =
        jack::Client::new("midi_controller", jack::ClientOptions::NO_START_SERVER).unwrap();

    let (sender, receiver) = sync_channel(64);

    std::thread::spawn(move || {
        let midi_controller = client
            .register_port("midi_loop_controller", jack::MidiIn::default())
            .unwrap();

        let cback = move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            let iter = midi_controller.iter(ps);
            for event in iter {
                let message: MidiEvent = event.into();
                let _ = sender.try_send(message);
            }
            jack::Control::Continue
        };

        let _active_client = client
            .activate_async((), jack::ClosureProcessHandler::new(cback))
            .unwrap();

        loop {
            std::thread::park();
        }
    });

    receiver
}

use std::convert::From;
use std::sync::mpsc::sync_channel;

/// Container for data of a midi signal
#[derive(Copy, Clone)]
pub struct MidiEvent {
    pub len: usize,
    pub data: [u8; 3],
    pub time: jack::Frames,
}

impl From<jack::RawMidi<'_>> for MidiEvent {
    fn from(midi: jack::RawMidi<'_>) -> Self {
        let len = std::cmp::min(3, midi.bytes.len());
        let mut data = [0; 3];
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

/// Non-blocking function to start listening for MidiEvents. Sends events
/// through channel associated with the returned receiver.
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

        // client.activate_async is non-blocking and if this thread terminates the
        // client gets dropped. This thread is done working so just park it until
        // the program is done.
        loop {
            std::thread::park();
        }
    });

    receiver
}

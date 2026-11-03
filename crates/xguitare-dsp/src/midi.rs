const STATUS_MASK: u8 = 0xF0;
const PROGRAM_CHANGE: u8 = 0xC0;
const DATA_MASK: u8 = 0x7F;

/// Returns the program number of a MIDI program change message on any channel, or `None` for any other message.
pub fn program_change(bytes: &[u8]) -> Option<u8> {
    match bytes {
        [status, program, ..] if status & STATUS_MASK == PROGRAM_CHANGE => {
            Some(program & DATA_MASK)
        }
        _ => None,
    }
}

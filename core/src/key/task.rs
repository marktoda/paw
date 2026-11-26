//! What the key needs of the task it works for: the task's handle, which
//! the session, the touch watch and the signer call back into. The task
//! itself, and the stages it shows, are [`crate::task`]'s.

use zeroize::Zeroizing;

use super::session::{Halt, KeySession, PinPrompt};
use super::KeyError;

/// A running task as the key's own code sees it, whatever it is for.
pub(crate) trait TaskSide: Send + Sync {
    fn cancelled(&self) -> bool;
    /// "#3": the task in the key log.
    fn tag(&self) -> String;
    /// Counts one more loss of the PIN's verification; returns how many
    /// this task has seen.
    fn note_pin_lost(&self) -> u32;
    fn wake(&self);
    fn show_plug_in(&self);
    fn show_busy(&self);
    fn show_touch(&self);
    fn show_working(&self);
    /// Waits until `done` says so, or the task is cancelled.
    fn wait_until(&self, done: &mut dyn FnMut() -> bool) -> Result<(), Halt>;
    /// Shows the PIN stage (`last_error` says why it is asked again) and
    /// waits for what the person types. Fails if the task is cancelled or
    /// `session`'s key goes away first.
    fn ask_pin(
        &self,
        session: &KeySession,
        prompt: PinPrompt,
        last_error: Option<KeyError>,
    ) -> Result<Zeroizing<Vec<u8>>, Halt>;
}

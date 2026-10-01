// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

use crossbeam_channel::{Sender, TryRecvError};

use crate::audiodevice::AudioMessage;
use crate::utils::resampling::ChunkResampler;
use crate::{CommandMessage, StatusMessage};

/// What a capture loop should do after [`handle_capture_command`].
#[derive(Debug, PartialEq)]
pub enum CommandOutcome {
    /// Keep capturing.
    Continue,
    /// The loop should set the device pitch to this speed, then keep capturing. Nothing has been
    /// applied to the device yet. Only returned when the device has a pitch control.
    SetPitch(f64),
    /// The engine asked capture to stop. The loop ends the stream, normally with
    /// [`send_capture_done`].
    Exit,
    /// The command channel is closed, so no Exit can arrive any more. The loop should end.
    Disconnected,
}

/// Handle one result of `try_recv` on a capture thread's command channel.
///
/// `SetSpeed` updates `rate_adjust`. When the device can adjust its own clock (`device_pitch`)
/// the speed is returned as [`CommandOutcome::SetPitch`] for the loop to apply, otherwise it goes
/// to the resampler here if that is asynchronous.
pub fn handle_capture_command(
    command: Result<CommandMessage, TryRecvError>,
    rate_adjust: &mut f64,
    resampler: &mut Option<ChunkResampler>,
    async_src: bool,
    device_pitch: bool,
) -> CommandOutcome {
    match command {
        Ok(CommandMessage::Exit) => {
            debug!("Exit message received.");
            CommandOutcome::Exit
        }
        Ok(CommandMessage::SetSpeed { speed }) => {
            debug!("Requested to adjust capture speed to {speed}.");
            *rate_adjust = speed;
            if device_pitch {
                return CommandOutcome::SetPitch(speed);
            } else if let Some(resampler) = resampler {
                if async_src {
                    if resampler.set_resample_ratio_relative(speed, true).is_err() {
                        debug!("Failed to set resampling speed to {speed}.");
                    }
                } else {
                    warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                }
            }
            CommandOutcome::Continue
        }
        Err(TryRecvError::Empty) => CommandOutcome::Continue,
        Err(TryRecvError::Disconnected) => {
            error!("Command channel was closed.");
            CommandOutcome::Disconnected
        }
    }
}

/// End the capture stream after an Exit command. `EndOfStream` goes to the processing thread
/// first, then `CaptureDone` to the engine.
pub fn send_capture_done(audio: &Sender<AudioMessage>, status: &Sender<StatusMessage>) {
    debug!("Sending EndOfStream.");
    audio.send(AudioMessage::EndOfStream).unwrap_or(());
    status.send(StatusMessage::CaptureDone).unwrap_or(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_and_disconnect_end_the_loop() {
        let mut rate_adjust = 1.0;
        let mut resampler = None;
        let outcome = handle_capture_command(
            Ok(CommandMessage::Exit),
            &mut rate_adjust,
            &mut resampler,
            true,
            false,
        );
        assert_eq!(outcome, CommandOutcome::Exit);
        let outcome = handle_capture_command(
            Err(TryRecvError::Disconnected),
            &mut rate_adjust,
            &mut resampler,
            true,
            false,
        );
        assert_eq!(outcome, CommandOutcome::Disconnected);
        let outcome = handle_capture_command(
            Err(TryRecvError::Empty),
            &mut rate_adjust,
            &mut resampler,
            true,
            false,
        );
        assert_eq!(outcome, CommandOutcome::Continue);
        assert_eq!(rate_adjust, 1.0);
    }

    #[test]
    fn set_speed_goes_to_the_pitch_control() {
        let mut rate_adjust = 1.0;
        let mut resampler = None;
        let outcome = handle_capture_command(
            Ok(CommandMessage::SetSpeed { speed: 1.001 }),
            &mut rate_adjust,
            &mut resampler,
            true,
            true,
        );
        assert_eq!(outcome, CommandOutcome::SetPitch(1.001));
        assert_eq!(rate_adjust, 1.001);
    }

    #[test]
    fn set_speed_without_pitch_or_resampler_only_records_it() {
        let mut rate_adjust = 1.0;
        let mut resampler = None;
        let outcome = handle_capture_command(
            Ok(CommandMessage::SetSpeed { speed: 0.999 }),
            &mut rate_adjust,
            &mut resampler,
            true,
            false,
        );
        assert_eq!(outcome, CommandOutcome::Continue);
        assert_eq!(rate_adjust, 0.999);
    }

    #[test]
    fn capture_done_sends_end_of_stream_first() {
        let (tx_audio, rx_audio) = crossbeam_channel::unbounded();
        let (tx_status, rx_status) = crossbeam_channel::unbounded();
        send_capture_done(&tx_audio, &tx_status);
        assert!(matches!(rx_audio.try_recv(), Ok(AudioMessage::EndOfStream)));
        assert!(matches!(
            rx_status.try_recv(),
            Ok(StatusMessage::CaptureDone)
        ));
    }
}

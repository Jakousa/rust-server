//! Client-side prediction buffer and reconciliation replay loop.
//!
//! Tracks local unacknowledged movement inputs with monotonic sequence tags, predicts local
//! character motion immediately, and authoritatively reconciles against server updates.
//! Divergences exceeding error tolerance ($\epsilon > 0.05\text{m}$) replay unacknowledged inputs
//! and smoothly bleed out positional error offsets over an exponential decay convergence window.

use std::collections::VecDeque;

/// A recorded client-side input with predicted displacement.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientInput {
    pub sequence: u32,
    pub predicted_position: (f32, f32, f32),
    pub input_displacement: (f32, f32, f32),
    pub dt_sec: f32,
    pub timestamp_sec: f64,
}

/// Client prediction buffer managing unacknowledged movement inputs,
/// authoritative server reconciliation replay, and smooth visual error offset convergence.
#[derive(Debug, Clone)]
pub struct PredictionBuffer {
    /// FIFO queue of unacknowledged movement inputs.
    pub unacknowledged_inputs: VecDeque<ClientInput>,
    /// Visual smoothing error offset currently decaying toward zero.
    pub visual_error_offset: (f32, f32, f32),
    /// Authoritative simulation position (base server state + replayed inputs).
    pub current_sim_pos: (f32, f32, f32),
    /// Exponential decay convergence rate (half-life factor, default 15.0/sec).
    pub convergence_speed: f32,
    /// Distance threshold for hard snapback (default 20.0m).
    pub hard_snap_threshold: f32,
    /// Positional error tolerance below which no replay occurs (default 0.05m = 5cm).
    pub tolerance: f32,
    /// Maximum unacknowledged inputs retained before pruning oldest.
    pub max_inputs: usize,
}

impl PredictionBuffer {
    pub const DEFAULT_CONVERGENCE_SPEED: f32 = 15.0;
    pub const DEFAULT_HARD_SNAP_THRESHOLD: f32 = 20.0;
    pub const DEFAULT_TOLERANCE: f32 = 0.05;
    pub const DEFAULT_MAX_INPUTS: usize = 64;

    /// Creates a new `PredictionBuffer` with default parameters.
    pub fn new() -> Self {
        Self {
            unacknowledged_inputs: VecDeque::with_capacity(Self::DEFAULT_MAX_INPUTS),
            visual_error_offset: (0.0, 0.0, 0.0),
            current_sim_pos: (0.0, 0.0, 0.0),
            convergence_speed: Self::DEFAULT_CONVERGENCE_SPEED,
            hard_snap_threshold: Self::DEFAULT_HARD_SNAP_THRESHOLD,
            tolerance: Self::DEFAULT_TOLERANCE,
            max_inputs: Self::DEFAULT_MAX_INPUTS,
        }
    }

    /// Pushes a newly executed client input into the unacknowledged queue.
    pub fn push_input(
        &mut self,
        sequence: u32,
        predicted_pos: (f32, f32, f32),
        displacement: (f32, f32, f32),
        dt_sec: f32,
        timestamp_sec: f64,
    ) {
        if self.unacknowledged_inputs.len() >= self.max_inputs {
            self.unacknowledged_inputs.pop_front();
        }
        self.unacknowledged_inputs.push_back(ClientInput {
            sequence,
            predicted_position: predicted_pos,
            input_displacement: displacement,
            dt_sec,
            timestamp_sec,
        });
        self.current_sim_pos = predicted_pos;
    }

    /// Returns the number of currently unacknowledged client inputs.
    pub fn pending_input_count(&self) -> usize {
        self.unacknowledged_inputs.len()
    }

    /// Reconciles the client's predicted position against an authoritative server state update
    /// acknowledging `ack_sequence`.
    ///
    /// If error $\le \text{tolerance}$ ($0.05\text{m}$), preserves predicted state.
    /// If error $> \text{hard\_snap\_threshold}$ ($20.0\text{m}$), hard-snaps simulation and visual state.
    /// Otherwise, replays remaining unacknowledged inputs from `server_pos` and smoothly decays
    /// the visual error offset.
    pub fn reconcile(&mut self, ack_sequence: u32, server_pos: (f32, f32, f32)) -> (f32, f32, f32) {
        let mut predicted_pos = None;
        let mut index_to_remove = None;

        for (index, input) in self.unacknowledged_inputs.iter().enumerate() {
            if input.sequence <= ack_sequence {
                index_to_remove = Some(index);
            }
            if input.sequence == ack_sequence {
                predicted_pos = Some(input.predicted_position);
                break;
            }
        }

        // Evict acknowledged inputs
        if let Some(index) = index_to_remove {
            self.unacknowledged_inputs.drain(..=index);
        }

        if let Some(pred_pos) = predicted_pos {
            let dx = server_pos.0 - pred_pos.0;
            let dy = server_pos.1 - pred_pos.1;
            let dz = server_pos.2 - pred_pos.2;
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();

            if distance <= self.tolerance {
                // Prediction was accurate within tolerance
                return self.current_sim_pos;
            } else if distance > self.hard_snap_threshold {
                // Hard snapback
                self.unacknowledged_inputs.clear();
                self.visual_error_offset = (0.0, 0.0, 0.0);
                self.current_sim_pos = server_pos;
                return server_pos;
            } else {
                // Soft divergence: replay unacknowledged inputs from server position
                let prev_visual = (
                    self.current_sim_pos.0 + self.visual_error_offset.0,
                    self.current_sim_pos.1 + self.visual_error_offset.1,
                    self.current_sim_pos.2 + self.visual_error_offset.2,
                );
                let mut replayed = server_pos;
                for input in &mut self.unacknowledged_inputs {
                    replayed.0 += input.input_displacement.0;
                    replayed.1 += input.input_displacement.1;
                    replayed.2 += input.input_displacement.2;
                    input.predicted_position = replayed;
                }
                self.current_sim_pos = replayed;
                self.visual_error_offset = (
                    prev_visual.0 - replayed.0,
                    prev_visual.1 - replayed.1,
                    prev_visual.2 - replayed.2,
                );
                return self.current_sim_pos;
            }
        } else {
            // Sequence not found in buffer (older than buffer capacity)
            let dx = server_pos.0 - self.current_sim_pos.0;
            let dy = server_pos.1 - self.current_sim_pos.1;
            let dz = server_pos.2 - self.current_sim_pos.2;
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();

            if distance > self.tolerance {
                self.unacknowledged_inputs.clear();
                self.visual_error_offset = (0.0, 0.0, 0.0);
                self.current_sim_pos = server_pos;
                return server_pos;
            }
        }

        self.current_sim_pos
    }

    /// Advances simulation frame time and exponentially decays visual error offset toward zero.
    pub fn update(&mut self, dt_sec: f32) {
        let decay_factor = (-self.convergence_speed * dt_sec).exp();
        self.visual_error_offset = (
            self.visual_error_offset.0 * decay_factor,
            self.visual_error_offset.1 * decay_factor,
            self.visual_error_offset.2 * decay_factor,
        );

        if self.visual_error_offset.0.abs() < 1e-4
            && self.visual_error_offset.1.abs() < 1e-4
            && self.visual_error_offset.2.abs() < 1e-4
        {
            self.visual_error_offset = (0.0, 0.0, 0.0);
        }
    }

    /// Returns the smoothed visual rendering position adding visual error offset to `base_pos`.
    pub fn visual_position(&self, base_pos: (f32, f32, f32)) -> (f32, f32, f32) {
        (
            base_pos.0 + self.visual_error_offset.0,
            base_pos.1 + self.visual_error_offset.1,
            base_pos.2 + self.visual_error_offset.2,
        )
    }

    /// Resets the prediction buffer state to stationary `pos`.
    pub fn reset(&mut self, pos: (f32, f32, f32)) {
        self.unacknowledged_inputs.clear();
        self.visual_error_offset = (0.0, 0.0, 0.0);
        self.current_sim_pos = pos;
    }
}

impl Default for PredictionBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fifo_input_queue_behavior_and_capacity_truncation() {
        let mut buffer = PredictionBuffer::new();
        for i in 0..65 {
            buffer.push_input(i as u32, (i as f32, 0.0, 0.0), (1.0, 0.0, 0.0), 0.1, 0.0);
        }
        assert_eq!(buffer.unacknowledged_inputs.len(), 64);
        assert_eq!(buffer.unacknowledged_inputs[0].sequence, 1);
        assert_eq!(buffer.unacknowledged_inputs[63].sequence, 64);
    }

    #[test]
    fn test_sub_tolerance_reconciliation() {
        let mut buffer = PredictionBuffer::new();
        buffer.push_input(1, (10.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.0);
        // Error of 0.03m <= 0.05m tolerance
        let pos = buffer.reconcile(1, (10.03, 0.0, 0.0));
        assert_eq!(pos, (10.0, 0.0, 0.0));
        assert_eq!(buffer.visual_error_offset, (0.0, 0.0, 0.0));
    }

    #[test]
    fn test_divergence_reconciliation_and_input_replay() {
        let mut buffer = PredictionBuffer::new();
        // Client executes 3 steps
        buffer.push_input(1, (1.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.0);
        buffer.push_input(2, (2.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.05);
        buffer.push_input(3, (3.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.10);

        // Server says at sequence 1, position was (0.5, 0.0, 0.0) -> divergence of 0.5m > 0.05m
        let reconciled_pos = buffer.reconcile(1, (0.5, 0.0, 0.0));

        // Replayed inputs 2 and 3: 0.5 + 1.0 + 1.0 = 2.5
        assert!((reconciled_pos.0 - 2.5).abs() < 1e-4);
        assert_eq!(buffer.unacknowledged_inputs.len(), 2);
        assert_eq!(buffer.unacknowledged_inputs[0].sequence, 2);
        assert!((buffer.unacknowledged_inputs[0].predicted_position.0 - 1.5).abs() < 1e-4);
        assert_eq!(buffer.unacknowledged_inputs[1].sequence, 3);
        assert!((buffer.unacknowledged_inputs[1].predicted_position.0 - 2.5).abs() < 1e-4);

        // Visual smoothing offset created: previous visual was 3.0, new sim is 2.5 -> error offset +0.5
        assert!((buffer.visual_error_offset.0 - 0.5).abs() < 1e-4);

        // Visual position initially equals old visual position (3.0) avoiding camera popping!
        let visual = buffer.visual_position(reconciled_pos);
        assert!((visual.0 - 3.0).abs() < 1e-4);

        // Advance 100ms: error offset decays smoothly
        buffer.update(0.10);
        assert!(buffer.visual_error_offset.0 < 0.5);
        assert!(buffer.visual_error_offset.0 > 0.0);

        // Further frames decay to 0
        buffer.update(1.0);
        assert_eq!(buffer.visual_error_offset.0, 0.0);
    }

    #[test]
    fn test_hard_snapback_on_extreme_teleport() {
        let mut buffer = PredictionBuffer::new();
        buffer.push_input(1, (0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.0);
        buffer.push_input(2, (1.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.05, 0.05);

        // Server reports teleport rollback to (50.0, 0.0, 0.0) -> divergence 50m > 20m
        let pos = buffer.reconcile(1, (50.0, 0.0, 0.0));
        assert_eq!(pos, (50.0, 0.0, 0.0));
        assert!(buffer.unacknowledged_inputs.is_empty());
        assert_eq!(buffer.visual_error_offset, (0.0, 0.0, 0.0));
    }
}

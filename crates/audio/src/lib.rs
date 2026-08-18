use litedroid_devices::VirtDevice;
use tracing::trace;

// ---------------------------------------------------------------------------
// AudioBackend
// ---------------------------------------------------------------------------

/// Available audio backends.
#[allow(unused)]
#[derive(Debug, Clone)]
pub enum AudioBackend {
    /// PulseAudio server (identified by its name, e.g. "default").
    PulseAudio(String),
    /// No audio output.
    Disabled,
}

// ---------------------------------------------------------------------------
// AudioDevice
// ---------------------------------------------------------------------------

const AUDIO_BUF_SIZE: usize = 4096;
const FLUSH_THRESHOLD: usize = 2048;

/// MMIO register offsets for the audio device.
const REG_CTRL: u64 = 0x00;
const REG_DATA: u64 = 0x04;
const REG_SAMPLE_RATE: u64 = 0x08;
const REG_CHANNELS: u64 = 0x0C;
const REG_STATUS: u64 = 0x10;

/// Simplified virtio audio device with a software-mixed internal buffer.
///
/// When PulseAudio is not available, writes to the DATA register accumulate in
/// an internal 4 KiB ring-buffer. Once 2 KiB or more have been buffered the
/// contents are discarded and a `trace!` log is emitted, simulating playback.
pub struct AudioDevice {
    buffer: Vec<u8>,
    sample_rate: u32,
    channels: u32,
    status: u32,
}

impl AudioDevice {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(AUDIO_BUF_SIZE),
            sample_rate: 48000,
            channels: 2,
            status: 0,
        }
    }

    fn flush_buffer(&mut self) {
        if self.buffer.len() >= FLUSH_THRESHOLD {
            trace!(
                bytes = self.buffer.len(),
                sample_rate = self.sample_rate,
                channels = self.channels,
                "audio playback: flushing buffer"
            );
            self.buffer.clear();
        }
    }
}

impl Default for AudioDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtDevice for AudioDevice {
    fn name(&self) -> &str {
        "virtio-audio"
    }

    fn device_type(&self) -> &str {
        "audio"
    }

    fn mmio_read(&mut self, offset: u64, _size: u32) -> u64 {
        match offset {
            REG_CTRL => 0,
            REG_DATA => 0, // simulated mic input — silence
            REG_SAMPLE_RATE => self.sample_rate as u64,
            REG_CHANNELS => self.channels as u64,
            REG_STATUS => self.status as u64,
            _ => 0,
        }
    }

    #[allow(unused)]
    fn mmio_write(&mut self, offset: u64, _size: u32, value: u64) {
        match offset {
            REG_CTRL => {
                // Future: start/stop/reset commands
            }
            REG_DATA => {
                if self.buffer.len() < AUDIO_BUF_SIZE {
                    self.buffer.push(value as u8);
                }
                self.flush_buffer();
            }
            REG_SAMPLE_RATE => {
                self.sample_rate = value as u32;
            }
            REG_CHANNELS => {
                self.channels = value as u32;
            }
            REG_STATUS => {
                self.status = value as u32;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.sample_rate = 48000;
        self.channels = 2;
        self.status = 0;
    }

    fn device_tree_compatible(&self) -> &str {
        "virtio,audio"
    }
}

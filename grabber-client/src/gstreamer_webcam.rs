use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use tokio::sync::mpsc;
use tracing::warn;

pub struct GStreamerWebcam {
    pipeline: gst::Pipeline,
}

impl GStreamerWebcam {
    pub fn new(camera_index: usize, width: u32, height: u32, fps: u32) -> Result<Self> {
        gst::init().context("Failed to initialize GStreamer")?;

        #[cfg(target_os = "macos")]
        let pipeline_str = format!(
            "avfvideosrc device-index={} ! \
             video/x-raw,format=NV12,width={},height={},framerate={}/1 ! \
             vtenc_h264 realtime=true allow-frame-reordering=false max-keyframe-interval=30 quality=0.7 ! \
             h264parse config-interval=1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink sync=false emit-signals=true",
            camera_index,
            width,
            height,
            fps,
        );

        #[cfg(target_os = "linux")]
        let pipeline_str = format!(
            "v4l2src device=/dev/video{} ! \
             video/x-raw,width={},height={},framerate={}/1 ! \
             videoconvert ! \
             vaapih264enc bitrate={} keyframe-period={} ! \
             h264parse ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink sync=false",
            camera_index,
            width,
            height,
            fps,
            3000,
            fps * 2
        );

        #[cfg(target_os = "windows")]
        let pipeline_str = format!(
            "mfvideosrc device-index={} ! \
             video/x-raw,width={},height={},framerate={}/1 ! \
             videoconvert ! \
             mfh264enc bitrate={} gop-size={} ! \
             h264parse config-interval=1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink sync=false emit-signals=true",
            camera_index,
            width,
            height,
            fps,
            15000,
            fps * 2
        );

        let pipeline = gst::parse::launch(&pipeline_str)
            .context("Failed to create GStreamer pipeline")?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("Failed to cast to Pipeline"))?;

        Ok(Self { pipeline })
    }

    pub async fn start_capture(self, frame_tx: mpsc::UnboundedSender<Vec<u8>>) -> Result<()> {
        let pipeline = self.pipeline;

        let appsink = pipeline
            .by_name("sink")
            .context("Failed to get appsink")?
            .dynamic_cast::<gst_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("Failed to cast to AppSink"))?;

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let data = map.as_slice().to_vec();

                    if frame_tx.send(data).is_err() {
                        return Err(gst::FlowError::Error);
                    }

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        pipeline
            .set_state(gst::State::Playing)
            .context("Failed to set pipeline to Playing")?;

        let bus = pipeline.bus().context("Pipeline without bus")?;

        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            use gst::MessageView;

            match msg.view() {
                MessageView::Eos(..) => break,
                MessageView::Error(err) => {
                    warn!(
                        "GStreamer error from {:?}: {}",
                        err.src().map(|s| s.path_string()),
                        err.error()
                    );
                    break;
                }
                _ => (),
            }
        }

        pipeline
            .set_state(gst::State::Null)
            .context("Failed to set pipeline to Null")?;

        Ok(())
    }
}

pub fn list_cameras() -> Result<Vec<String>> {
    gst::init().context("Failed to initialize GStreamer")?;

    #[cfg(target_os = "macos")]
    {
        let mut cameras = Vec::new();
        for i in 0..10 {
            let pipeline_str = format!("avfvideosrc device-index={} ! fakesink", i);

            if let Ok(pipeline) = gst::parse::launch(&pipeline_str) {
                if pipeline.set_state(gst::State::Ready).is_ok() {
                    cameras.push(format!("Camera {}: AVFoundation device", i));
                    let _ = pipeline.set_state(gst::State::Null);
                }
            }
        }
        Ok(cameras)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(vec![
            "Camera listing not implemented for this platform".to_string()
        ])
    }
}

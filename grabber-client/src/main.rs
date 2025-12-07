mod gstreamer_webcam;
mod webrtc_publisher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "grabber-client")]
#[command(about = "Native WebRTC Grabber Client for screen and webcam capture")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    List {
        #[arg(value_enum, default_value = "all")]
        device: DeviceType,
    },

    Screen {
        #[arg(short, long, default_value = "ws://localhost:3000/ws/grabber")]
        url: String,

        #[arg(short, long, default_value = "test")]
        credential: String,

        #[arg(short, long, default_value = "0")]
        display: usize,

        #[arg(short, long, default_value = "30")]
        fps: u32,
    },

    Webcam {
        #[arg(short, long, default_value = "ws://localhost:3000/ws/grabber")]
        url: String,

        #[arg(long, default_value = "test")]
        credential: String,

        #[arg(long, default_value = "0")]
        camera: usize,

        #[arg(long, default_value = "1280")]
        width: u32,

        #[arg(long, default_value = "720")]
        height: u32,

        #[arg(short, long, default_value = "30")]
        fps: u32,
    },

    Both {
        #[arg(long, default_value = "ws://localhost:3000/ws/grabber")]
        url: String,

        #[arg(default_value = "test")]
        credential: String,

        #[arg(long, default_value = "0")]
        display: usize,

        #[arg(long, default_value = "0")]
        camera: usize,

        #[arg(long, default_value = "1280")]
        width: u32,

        #[arg(long, default_value = "720")]
        height: u32,

        #[arg(long, default_value = "30")]
        fps: u32,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum DeviceType {
    Screen,
    Webcam,
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::List { device } => handle_list(device),
        Commands::Screen {
            url: _,
            credential: _,
            display: _,
            fps: _,
        } => {
            eprintln!("Screen capture is temporarily disabled");
            Ok(())
        }
        Commands::Webcam {
            url,
            credential,
            camera,
            width,
            height,
            fps,
        } => handle_webcam_gst_capture(url, credential, camera, width, height, fps).await,
        Commands::Both {
            url: _,
            credential: _,
            display: _,
            camera: _,
            width: _,
            height: _,
            fps: _,
        } => {
            eprintln!("Both capture is temporarily disabled");
            Ok(())
        }
    }
}

fn handle_list(device_type: DeviceType) -> Result<()> {
    match device_type {
        DeviceType::Screen | DeviceType::All => {
            println!("\n=== Available Displays ===");
            println!("  Screen capture is temporarily disabled");
        }
        _ => {}
    }

    match device_type {
        DeviceType::Webcam | DeviceType::All => {
            println!("\n=== Available Cameras ===");
            match gstreamer_webcam::list_cameras() {
                Ok(cameras) => {
                    for camera in cameras {
                        println!("  {}", camera);
                    }
                }
                Err(e) => eprintln!("Error listing cameras: {}", e),
            }
        }
        _ => {}
    }

    println!();
    Ok(())
}

async fn handle_webcam_gst_capture(
    url: String,
    credential: String,
    camera_index: usize,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<()> {
    let capturer = gstreamer_webcam::GStreamerWebcam::new(camera_index, width, height, fps)?;
    let mut publisher = webrtc_publisher::WebRTCPublisher::new(url, credential);
    let frame_tx = publisher.connect_and_publish(width, height).await?;
    capturer.start_capture(frame_tx).await?;
    Ok(())
}

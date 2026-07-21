use std::{io::Cursor, os::fd::OwnedFd, path::Path};

use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;

const TOKEN_FILE: &str = "restore_token.txt";

#[derive(Default)]
struct State {
    format: spa::param::video::VideoInfoRaw,
    saved: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    let (node_id, fd) = rt.block_on(portal_session())?;
    eprintln!("portal ok: node_id={node_id}, starting pipewire consumer");
    consume(node_id, fd)
}

async fn portal_session() -> Result<(u32, OwnedFd), Box<dyn std::error::Error>> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session().await?;
    let restore_token = std::fs::read_to_string(TOKEN_FILE).ok();
    proxy
        .select_sources(
            &session,
            CursorMode::Hidden,
            SourceType::Window | SourceType::Monitor,
            false,
            restore_token.as_deref(),
            PersistMode::ExplicitlyRevoked,
        )
        .await?;
    let response = proxy.start(&session, None).await?.response()?;
    if let Some(token) = response.restore_token() {
        std::fs::write(TOKEN_FILE, token)?;
    }
    let stream = response
        .streams()
        .first()
        .ok_or("portal returned no streams")?;
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy.open_pipe_wire_remote(&session).await?;
    Ok((node_id, fd))
}

fn consume(node_id: u32, fd: OwnedFd) -> Result<(), Box<dyn std::error::Error>> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "poe2-lens-spike-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(State::default())
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            if state.format.parse(param).is_ok() {
                eprintln!(
                    "negotiated {}x{} format {:?}",
                    state.format.size().width,
                    state.format.size().height,
                    state.format.format()
                );
            }
        })
        .process(|stream, state| {
            if state.saved {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            let Some(bytes) = data.data() else { return };
            let (w, h) = (state.format.size().width, state.format.size().height);
            if w == 0 || h == 0 {
                return;
            }
            // BGRx/BGRA -> RGB
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for px in bytes.chunks_exact(4).take((w * h) as usize) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            image::RgbImage::from_raw(w, h, rgb)
                .expect("buffer size mismatch")
                .save(Path::new("frame.png"))
                .expect("failed to save frame.png");
            eprintln!("saved frame.png ({w}x{h})");
            state.saved = true;
            std::process::exit(0);
        })
        .register()?;

    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 2560, height: 1440 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let values = spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or("bad pod")?];

    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    mainloop.run();
    Ok(())
}

//! Loopback integration test: full server ↔ client conversation over a real
//! TCP socket — handshake, frames, PING/PONG, FRAME_ACK, graceful disconnect,
//! and reconnect semantics.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use usbra_host::server::{self, ServeConfig};
use usbra_protocol as proto;

fn test_config() -> ServeConfig {
    ServeConfig {
        bind: "127.0.0.1".into(),
        port: 0, // ephemeral
        width: 640,
        height: 360,
        fps: 60,
        full_every_secs: 0, // damage-only after the first full frame
        stats_path: None,
        frame_ack: true,
        source: "test".into(),
    }
}

#[test]
fn handshake_frames_ping_ack_disconnect() {
    let cfg = test_config();
    let (addr, _handle) = server::spawn(&cfg).expect("spawn server");

    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_nodelay(true).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    // --- HELLO ---
    proto::write_message(
        &mut s,
        &proto::Message::Hello(proto::Hello {
            proto_version: proto::VERSION,
            capabilities: proto::caps::RAW,
            screen_w: 1080,
            screen_h: 2340,
            refresh_hint: 60,
            name: "loopback-test".into(),
        }),
    )
    .unwrap();

    // --- handshake replies: HELLO_OK, DISPLAY_INFO, CONFIG ---
    let mut session_id = 0u64;
    let mut got = [false; 3];
    for _ in 0..3 {
        match proto::read_message(&mut s).unwrap().expect("handshake message") {
            proto::Message::HelloOk(h) => {
                got[0] = true;
                session_id = h.session_id;
                assert_eq!(h.proto_version, proto::VERSION);
                assert_ne!(session_id, 0);
                assert_eq!(h.capabilities, proto::caps::RAW);
            }
            proto::Message::DisplayInfo(d) => {
                got[1] = true;
                assert_eq!((d.width, d.height), (640, 360));
                assert_eq!(d.refresh, 60);
                assert_eq!(d.pixel_format, proto::pixel_format::BGRX);
            }
            proto::Message::Config(c) => {
                got[2] = true;
                assert_eq!(c.codec, proto::codec::RAW);
                assert_ne!(c.flags & proto::Config::FLAG_FRAME_ACK, 0);
            }
            m => panic!("unexpected during handshake: {:?}", m),
        }
    }
    assert!(got[0] && got[1] && got[2], "missing handshake messages: {got:?}");

    // --- frames: first must cover the display; later ones are damage ---
    let ping_ts = 123_456_789u64;
    let mut ping_sent = false;
    let mut pongs = 0;
    let mut frames = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    while frames < 10 && Instant::now() < deadline {
        match proto::read_message(&mut s).unwrap().expect("stream message") {
            proto::Message::Frame(f) => {
                frames += 1;
                let expect: usize = f.rects.iter().map(|r| r.byte_len() as usize).sum();
                assert_eq!(expect, f.payload.len(), "payload must match rect packing");
                if frames == 1 {
                    assert!(
                        f.covers(640, 360),
                        "first frame must be a full frame (got {} rects)",
                        f.rects.len()
                    );
                } else {
                    assert!(!f.rects.is_empty());
                    assert!(!f.covers(640, 360));
                }
                if !ping_sent && frames >= 5 {
                    proto::write_message(
                        &mut s,
                        &proto::Message::Ping(proto::Ping { client_ts_ns: ping_ts }),
                    )
                    .unwrap();
                    ping_sent = true;
                }
                if frames % 3 == 0 {
                    proto::write_message(
                        &mut s,
                        &proto::Message::FrameAck(proto::FrameAck {
                            frame_id: f.frame_id,
                            client_ts_ns: 0,
                        }),
                    )
                    .unwrap();
                }
            }
            proto::Message::Pong(p) => {
                assert_eq!(p.echoed_client_ts_ns, ping_ts);
                pongs += 1;
            }
            m => panic!("unexpected on stream: {:?}", m),
        }
    }
    assert!(frames >= 10, "received only {frames} frames");
    assert!(pongs >= 1, "no PONG received");

    // --- graceful DISCONNECT: server replies DISCONNECT then closes ---
    proto::write_message(
        &mut s,
        &proto::Message::Disconnect(proto::Disconnect {
            reason: proto::reason::SHUTDOWN,
            message: "test done".into(),
        }),
    )
    .unwrap();
    let mut saw_bye = false;
    let mut saw_eof = false;
    for _ in 0..100 {
        match proto::read_message(&mut s).unwrap() {
            Some(proto::Message::Disconnect(_)) => saw_bye = true,
            Some(proto::Message::Frame(_)) => continue, // in-flight frame before our Bye
            Some(m) => panic!("unexpected during close: {:?}", m),
            None => {
                saw_eof = true;
                break;
            }
        }
    }
    assert!(saw_bye || saw_eof, "server did not close the connection");

    // --- reconnect path: RECONNECT with a known session resumes ---
    let mut s2 = TcpStream::connect(addr).expect("reconnect");
    s2.set_nodelay(true).unwrap();
    s2.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    proto::write_message(
        &mut s2,
        &proto::Message::Reconnect(proto::Reconnect { session_id, last_frame_id: 9 }),
    )
    .unwrap();
    let mut resumed_ok = false;
    let mut got_full_frame = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match proto::read_message(&mut s2).unwrap().expect("resume message") {
            proto::Message::HelloOk(h) => {
                assert_eq!(h.session_id, session_id);
                resumed_ok = true;
            }
            proto::Message::DisplayInfo(_) | proto::Message::Config(_) => {}
            proto::Message::Frame(f) => {
                // resync must start with a full frame
                assert!(
                    f.covers(640, 360),
                    "post-reconnect first frame must be full (got {} rects)",
                    f.rects.len()
                );
                got_full_frame = true;
                break;
            }
            m => panic!("unexpected during resume: {:?}", m),
        }
    }
    assert!(resumed_ok, "server did not accept the RECONNECT");
    assert!(got_full_frame, "server did not resync with a full frame");

    // --- unknown session reconnect → DISCONNECT(UNKNOWN_SESSION) ---
    let mut s3 = TcpStream::connect(addr).expect("unknown-session connect");
    s3.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    proto::write_message(
        &mut s3,
        &proto::Message::Reconnect(proto::Reconnect { session_id: 0xFFFF_FFFF, last_frame_id: 0 }),
    )
    .unwrap();
    match proto::read_message(&mut s3).unwrap().expect("reject message") {
        proto::Message::Disconnect(d) => {
            assert_eq!(d.reason, proto::reason::UNKNOWN_SESSION);
        }
        m => panic!("expected UNKNOWN_SESSION disconnect, got {:?}", m),
    }
}

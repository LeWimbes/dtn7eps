use super::daemon::*;
use crate::core::core::{DtnCore, DtnPeer, PeerType};
use bp7::EndpointID;
use futures::{try_ready, Future, Poll};
use log::{debug, error, info, trace, warn};
use net2::UdpBuilder;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::prelude::*;
use tokio::timer::Interval;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnnouncementPkt {
    eid: Option<EndpointID>,
    cl: Vec<String>,
}
struct Server {
    socket: UdpSocket,
    buf: Vec<u8>,
    to_send: Option<(usize, SocketAddr)>,
    tx: Sender<DtnCmd>,
}

impl Future for Server {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> Poll<(), io::Error> {
        loop {
            if let Some((size, peer)) = Some(try_ready!(self.socket.poll_recv_from(&mut self.buf)))
            {
                let deserialized: AnnouncementPkt = match serde_cbor::from_slice(&self.buf[..size])
                {
                    Ok(pkt) => pkt,
                    Err(e) => {
                        error!("{}", e);
                        continue;
                    }
                };
                //let amt = try_ready!(self.socket.poll_send_to(&self.buf[..size], &peer));
                //println!("Echoed {}/{} bytes to {}", amt, size, peer);
                debug!("Packet from {} : {:?}", peer, deserialized);
                let dtnpeer = DtnPeer::new(
                    deserialized.eid,
                    peer.ip(),
                    PeerType::Dynamic,
                    deserialized.cl,
                );
                access_core(self.tx.clone(), |c| {
                    c.peers.insert(peer.ip(), dtnpeer.clone());
                });
                self.to_send = None;
            }
        }
    }
}

fn announcer(core: &mut DtnCore, socket: std::net::UdpSocket) {
    let sock = UdpSocket::from_std(socket, &tokio::reactor::Handle::default()).unwrap();
    debug!("running announcer");

    // Compile list of conversion layers as string vector
    let mut cls: Vec<String> = Vec::new();
    for cl in &core.cl_list {
        cls.push(cl.to_string());
    }
    //let addr = "127.0.0.1:3003".parse().unwrap();
    let addr = "224.0.0.26:3003".parse().unwrap();
    let pkt = AnnouncementPkt { eid: None, cl: cls };
    let anc = sock
        .send_dgram(serde_cbor::to_vec(&pkt).unwrap(), &addr)
        .and_then(|_| Ok(()))
        .map_err(|e| error!("{:?}", e));
    tokio::spawn(anc);
}
pub fn spawn_service_discovery(tx: Sender<DtnCmd>) {
    let addr: std::net::SocketAddr = "0.0.0.0:3003".parse().unwrap();
    let socket = UdpBuilder::new_v4().unwrap();
    socket.reuse_address(true).unwrap();
    let socket = socket.bind(addr).unwrap();
    socket
        .set_multicast_loop_v4(true)
        .expect("error activating multicast loop v4");
    socket
        .join_multicast_v4(
            &"224.0.0.26".parse().unwrap(),
            &std::net::Ipv4Addr::new(0, 0, 0, 0),
        )
        .expect("error joining multicast v4 group");
    let socket_clone = socket.try_clone().expect("couldn't clone the socket");
    let sock = UdpSocket::from_std(socket, &tokio::reactor::Handle::default()).unwrap();

    //let sock = UdpSocket::bind(&([0, 0, 0, 0], 3003).into()).unwrap();

    info!("Listening on {}", sock.local_addr().unwrap());
    let server = Server {
        socket: sock,
        buf: vec![0; 1024],
        to_send: None,
        tx: tx.clone(),
    };

    tokio::spawn(server.map_err(|e| println!("server error = {:?}", e)));

    let tx = std::sync::Mutex::new(tx.clone());
    let task = Interval::new(Instant::now(), Duration::from_millis(10000))
        .for_each(move |_instant| {
            access_core(tx.lock().unwrap().clone(), |c| {
                announcer(
                    c,
                    socket_clone.try_clone().expect("couldn't clone the socket"),
                );
            });
            Ok(())
        })
        .map_err(|e| panic!("interval errored; err={:?}", e));
    tokio::spawn(task);
}
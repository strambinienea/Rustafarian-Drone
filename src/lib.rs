#![allow(unused)]
use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use std::collections::{HashMap, HashSet};
use std::{fs, thread};
use wg_2024::config::Config;
use wg_2024::controller::{DroneCommand, NodeEvent};
use wg_2024::drone::Drone;
use wg_2024::drone::DroneOptions;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{FloodRequest, FloodResponse, NodeType};
use wg_2024::packet::{Packet, PacketType};
use rand::*;

pub struct RustafarianDrone {
    id: NodeId,
    controller_send: Sender<NodeEvent>,
    controller_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    pdr: f32,
    neighbors: HashMap<NodeId, Sender<Packet>>,
    flood_requests: HashSet<u64>, // Contains: O(1) in average
    crashed: bool,
}

impl Drone for RustafarianDrone {
    fn new(options: DroneOptions) -> Self {
        Self {
            id: options.id,
            controller_send: options.controller_send,
            controller_recv: options.controller_recv,
            packet_recv: options.packet_recv,
            pdr: options.pdr,
            neighbors: HashMap::new(),
            flood_requests: HashSet::new(),
            crashed: false
        }
    }

    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        if let DroneCommand::Crash = command {
                            println!("drone {} crashed", self.id);
                            break;
                        }
                        self.handle_command(command);
                    }
                }
                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(packet);
                    }
                },
            }
        }
    }
}

impl RustafarianDrone {
    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Nack(_nack) => todo!(),
            PacketType::Ack(_ack) => todo!(),
            PacketType::MsgFragment(_fragment) => todo!(),
            PacketType::FloodRequest(_flood_request) => {
                println!("Flood request arrived at {:?}", self.id);
                self.handle_flood_req(_flood_request, packet.session_id, packet.routing_header);
            }
            PacketType::FloodResponse(_flood_response) => {
                println!("Flood response arrived at {:?}", self.id);
            }
        }
    }

    fn handle_command(&mut self, command: DroneCommand) {
        match command {
            DroneCommand::AddSender(node_id, sender) => self.add_neighbor(node_id, sender),
            DroneCommand::SetPacketDropRate(pdr) => self.set_packet_drop_rate(pdr),
            DroneCommand::Crash => self.make_crash(),
        }
    }

    fn add_neighbor(&mut self, node_id: u8, neighbor: Sender<Packet>) {
        self.neighbors.insert(node_id, neighbor);
    }

    fn make_crash(&mut self) {
        self.crashed = true;
    }

    fn set_packet_drop_rate(&mut self, pdr: f32) {
        self.pdr = pdr;
    }

    fn should_drop(&self) -> bool {
        return rand::thread_rng().gen_range(0.0..100.0) < self.pdr;
    }

    fn forward_packet(&mut self, packet: Packet) {
        // Step 1: check I'm the intended receiver
        let curr_hop = packet.routing_header.hops[packet.routing_header.hop_index];
        if self.id != curr_hop {
            // Error, I'm not the one who's supposed to receive this
            todo!();
        }

        let mut new_packet = packet.clone();
        // Step 2: increase the hop index
        new_packet.routing_header.hop_index += 1;
        let next_hop_index = new_packet.routing_header.hop_index;
        
        // Step 3: check I'm not the last hop
        if next_hop_index >= packet.routing_header.hops.len() {
            // Error, I'm the last hop!
            todo!();
        }
        
        // Step 4: Check I have the next hop as neighbor
        let next_hop = packet.routing_header.hops[next_hop_index];

        match self.neighbors.get(&next_hop) {
            Some(channel) => {
                match channel.send(new_packet) {
                    Ok(()) => {},
                    Err(error) => {
                        // Error, I can't send the message
                        todo!();
                    }
                }
            },
            None => {
                // Error, I don't have that node
                todo!();
            }
        }
    }

    /**
     * When a flood request packet is received:
     * 1. The drone adds itself to the path_trace
     * 2.If the ID is in the memory: create and send a FloodResponse
     * 3. Otherwise:
     *  3.1 if has neighbors forwards the packet to its neighbors
     *  3.2 if no neighbors send it to node from which it received it
     */
    pub fn handle_flood_req(
        &mut self,
        packet: FloodRequest,
        session_id: u64,
        routing_header: SourceRoutingHeader,
    ) {
        // If we have the ID in memory, and the path trace contains our ID
        // Request already handled, prepare response
        /** && packet.clone().path_trace.iter().any(|node| node.0 == self.id) */
        if self.flood_requests.contains(&packet.flood_id) {
            let mut flood_request_clone = packet.clone();
            // Get the ID of the drone that sent the request
            let sender_id = flood_request_clone.path_trace.last().unwrap().0;
            // Add myself to the path trace
            flood_request_clone
                .path_trace
                .push((self.id, NodeType::Drone));
            let mut route: Vec<u8> = flood_request_clone
                .path_trace
                .clone()
                .into_iter()
                .map(|node| node.0)
                .collect();
            route.reverse();

            let response = FloodResponse {
                flood_id: packet.flood_id,
                path_trace: flood_request_clone.path_trace,
            };

            // Create the packet with the route provided in the path trace
            let new_packet = Packet {
                pack_type: PacketType::FloodResponse(response),
                session_id,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: route,
                },
            };

            match self.neighbors.get(&sender_id) {
                Some(channel) => match channel.send(new_packet) {
                    Ok(()) => {},
                    Err(error) => println!("Couldn't send response, as the neighbor has crashed")
                },
                _ => {}
            }
        } else {
            // Send to neighbors
            let mut new_packet = packet.clone();
            // Save the last node's ID, we don't want to send the request to it
            let last_node = (new_packet.path_trace.last().unwrap().0);
            // Add our ID to the trace
            new_packet.path_trace.push((self.id, NodeType::Drone));

            // Send to all neighbors
            for neighbor in &self.neighbors {
                let neighbor_id = neighbor.0;
                let neighbor_channel = neighbor.1;
                if neighbor_id == &last_node {
                    continue;
                }

                match neighbor_channel.send(Packet {
                    pack_type: PacketType::FloodRequest(new_packet.clone()),
                    routing_header: routing_header.clone(),
                    session_id,
                }) {
                    Ok(()) => {}
                    Err(error) => println!("Couldn't send response, as the neighbor has crashed")
                }
            }
        }
    }
}

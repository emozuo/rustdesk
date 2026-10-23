//! 打印本机物理网卡地址集合，用来在各台机器上核对判据（虚拟网卡不应出现）
//!
//!   cargo run -p ddpro-ident --example macs

fn main() {
    let macs = ddpro_ident::physical_macs();
    println!("{} physical: {}", macs.len(), ddpro_ident::to_line(&macs));
}

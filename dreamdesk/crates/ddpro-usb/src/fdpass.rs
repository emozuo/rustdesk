//! 主控端 Linux：经 Unix 套接字把一个文件描述符交给另一个进程（SCM_RIGHTS）。会话进程用它把本机套接字对的一端交给
//! root 端点，由 root 写进 `usbip_sockfd`（描述符必须在写的那个进程里）。写法同 RustDesk 的 `_drm` 通道（`src/ipc/drm.rs`）。

use std::{
    io,
    os::fd::{FromRawFd, OwnedFd, RawFd},
};

/// 一条消息，附带至多一个描述符
pub fn send(sock: RawFd, data: &[u8], fd: Option<RawFd>) -> io::Result<usize> {
    // cmsghdr 要按指针对齐
    let mut control = [0u64; 4];
    unsafe {
        let mut iov = libc::iovec {
            iov_base: data.as_ptr() as *mut libc::c_void,
            iov_len: data.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            let size = std::mem::size_of::<RawFd>() as u32;
            msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = libc::CMSG_SPACE(size) as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(size) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg) as *mut RawFd, fd);
        }
        let n = libc::sendmsg(sock, &msg, libc::MSG_NOSIGNAL);
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

/// 收一条消息；带着描述符的话一并取出（多出来的关掉）。0 字节表示对端关了
pub fn recv(sock: RawFd, buf: &mut [u8]) -> io::Result<(usize, Option<OwnedFd>)> {
    let mut control = [0u64; 8];
    unsafe {
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = std::mem::size_of_val(&control) as _;
        let n = libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut got: Option<OwnedFd> = None;
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg) as *const RawFd;
                let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let fd = OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i)));
                    if got.is_none() {
                        got = Some(fd);
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated SCM_RIGHTS control message"));
        }
        Ok((n as usize, got))
    }
}

// Take a look at the license at the top of the repository in the LICENSE file.

use crate::{
    common::{Gid, Uid},
    windows::sid::Sid,
    Group, UserExt,
};

use std::ptr::null_mut;

use winapi::{
    shared::{
        lmcons::{MAX_PREFERRED_LENGTH, NET_API_STATUS},
        minwindef::{DWORD, LPBYTE},
        ntdef::{LPWSTR, WCHAR},
        ntstatus::STATUS_SUCCESS,
        winerror::ERROR_MORE_DATA,
    },
    um::{
        lmaccess::{
            NetUserEnum, NetUserGetInfo, NetUserGetLocalGroups, FILTER_NORMAL_ACCOUNT,
            LG_INCLUDE_INDIRECT, LOCALGROUP_USERS_INFO_0, USER_INFO_0, USER_INFO_23,
        },
        lmapibuf::NetApiBufferFree,
        ntlsa::{
            LsaEnumerateLogonSessions, LsaFreeReturnBuffer, LsaGetLogonSessionData,
            SECURITY_LOGON_SESSION_DATA,
        },
        winnt::LUID,
    },
};
use windows::core::PWSTR;

#[doc = include_str!("../../md_doc/user.md")]
pub struct User {
    pub(crate) uid: Uid,
    pub(crate) gid: Gid,
    pub(crate) name: String,
    c_user_name: Vec<WCHAR>,
    is_local: bool,
}

impl User {
    fn new(uid: Uid, name: String, c_name: LPWSTR, is_local: bool) -> Self {
        unsafe {
            let c_user_name = if is_local {
                let mut i = 0;
                loop {
                    let c = *c_name.offset(i);
                    if c == 0 {
                        i += 1; // We increase by 1 because we need the '\0' at the end.
                        break;
                    }
                    i += 1;
                }
                Vec::from(std::slice::from_raw_parts(c_name, i as _))
            } else {
                // There is no local groups for a non-local user.
                Vec::new()
            };

            Self {
                uid,
                gid: Gid(0),
                name,
                c_user_name,
                is_local,
            }
        }
    }
}

impl UserExt for User {
    fn id(&self) -> &Uid {
        &self.uid
    }

    fn group_id(&self) -> Gid {
        self.gid
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn groups(&self) -> Vec<Group> {
        if self.is_local {
            unsafe { get_groups_for_user(self.c_user_name.as_ptr() as _) }
        } else {
            Vec::new()
        }
    }
}

// FIXME: Can be removed once merged in winapi.
#[allow(non_upper_case_globals)]
const NERR_Success: NET_API_STATUS = 0;

struct NetApiBuffer<T>(*mut T);

impl<T> Drop for NetApiBuffer<T> {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                NetApiBufferFree(self.0 as *mut _);
            }
        }
    }
}

impl<T> Default for NetApiBuffer<T> {
    fn default() -> Self {
        Self(null_mut())
    }
}

impl<T> NetApiBuffer<T> {
    pub fn inner_mut(&mut self) -> &mut *mut T {
        assert!(self.0.is_null());
        &mut self.0
    }

    pub unsafe fn inner_mut_as_bytes(&mut self) -> &mut LPBYTE {
        // https://doc.rust-lang.org/std/mem/fn.transmute.html
        // Turning an &mut T into an &mut U:
        &mut *(self.inner_mut() as *mut *mut T as *mut LPBYTE)
    }
}

struct LsaBuffer<T>(*mut T);

impl<T> Drop for LsaBuffer<T> {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                LsaFreeReturnBuffer(self.0 as *mut _);
            }
        }
    }
}

impl<T> Default for LsaBuffer<T> {
    fn default() -> Self {
        Self(null_mut())
    }
}

impl<T> LsaBuffer<T> {
    pub fn inner_mut(&mut self) -> &mut *mut T {
        assert!(self.0.is_null());
        &mut self.0
    }
}

unsafe fn get_groups_for_user(username: LPWSTR) -> Vec<Group> {
    let mut buf: NetApiBuffer<LOCALGROUP_USERS_INFO_0> = Default::default();
    let mut nb_entries = 0;
    let mut total_entries = 0;
    let mut groups;

    let status = NetUserGetLocalGroups(
        [0u16].as_ptr(),
        username,
        0,
        LG_INCLUDE_INDIRECT,
        buf.inner_mut_as_bytes(),
        MAX_PREFERRED_LENGTH,
        &mut nb_entries,
        &mut total_entries,
    );

    if status == NERR_Success {
        groups = Vec::with_capacity(nb_entries as _);
        if !buf.0.is_null() {
            let entries = std::slice::from_raw_parts(buf.0, nb_entries as _);
            groups.extend(entries.iter().map(|entry| Group {
                name: PWSTR(entry.lgrui0_name).to_string().unwrap(),
                id: Gid(0),
            }));
        }
    } else {
        groups = Vec::new();
        sysinfo_debug!("NetUserGetLocalGroups failed with ret code {}", status);
    }

    groups
}

pub unsafe fn get_users() -> Vec<User> {
    let mut users = Vec::new();

    let mut resume_handle: DWORD = 0;
    loop {
        let mut buffer: NetApiBuffer<USER_INFO_0> = Default::default();
        let mut nb_read = 0;
        let mut total = 0;
        let status = NetUserEnum(
            null_mut(),
            0,
            FILTER_NORMAL_ACCOUNT,
            buffer.inner_mut_as_bytes(),
            MAX_PREFERRED_LENGTH,
            &mut nb_read,
            &mut total,
            &mut resume_handle,
        );
        if status == NERR_Success || status == ERROR_MORE_DATA {
            let entries = std::slice::from_raw_parts(buffer.0, nb_read as _);
            for entry in entries {
                if entry.usri0_name.is_null() {
                    continue;
                }

                let mut user: NetApiBuffer<USER_INFO_23> = Default::default();
                if NetUserGetInfo(null_mut(), entry.usri0_name, 23, user.inner_mut_as_bytes())
                    == NERR_Success
                {
                    if let Some(sid) = Sid::from_psid((*user.0).usri23_user_sid) {
                        // Get the account name from the SID (because it's usually
                        // a better name), but fall back to the name we were given
                        // if this fails.
                        let name = sid
                            .account_name()
                            .unwrap_or_else(|| PWSTR(entry.usri0_name).to_string().unwrap());
                        users.push(User::new(Uid(sid), name, entry.usri0_name, true))
                    }
                }
            }
        } else {
            sysinfo_debug!(
                "NetUserEnum error: {}",
                if status == winapi::shared::winerror::ERROR_ACCESS_DENIED {
                    "access denied"
                } else if status == winapi::shared::winerror::ERROR_INVALID_LEVEL {
                    "invalid level"
                } else {
                    "unknown error"
                }
            );
        }
        if status != ERROR_MORE_DATA {
            break;
        }
    }

    // First part done. Second part now!
    let mut nb_sessions = 0;
    let mut uids: LsaBuffer<LUID> = Default::default();
    if LsaEnumerateLogonSessions(&mut nb_sessions, uids.inner_mut()) != STATUS_SUCCESS {
        sysinfo_debug!("LsaEnumerateLogonSessions failed");
    } else {
        let entries = std::slice::from_raw_parts_mut(uids.0, nb_sessions as _);
        for entry in entries {
            let mut data: LsaBuffer<SECURITY_LOGON_SESSION_DATA> = Default::default();
            if LsaGetLogonSessionData(entry, data.inner_mut()) == STATUS_SUCCESS
                && !data.0.is_null()
            {
                let data = *data.0;
                if data.LogonType == winapi::um::ntlsa::Network {
                    continue;
                }

                let sid = match Sid::from_psid(data.Sid) {
                    Some(sid) => sid,
                    None => continue,
                };

                if users.iter().any(|u| u.uid.0 == sid) {
                    continue;
                }

                // Get the account name from the SID (because it's usually
                // a better name), but fall back to the name we were given
                // if this fails.
                let name = sid.account_name().unwrap_or_else(|| {
                    String::from_utf16(std::slice::from_raw_parts(
                        data.UserName.Buffer,
                        data.UserName.Length as usize / std::mem::size_of::<u16>(),
                    ))
                    .unwrap_or_else(|_err| {
                        sysinfo_debug!("Failed to convert from UTF-16 string: {}", _err);
                        String::new()
                    })
                });

                users.push(User::new(Uid(sid), name, std::ptr::null_mut(), false));
            }
        }
    }

    users
}

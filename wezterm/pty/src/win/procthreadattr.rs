use crate::win::psuedocon::HPCON;
use anyhow::{ensure, Error};
use std::io::Error as IoError;
use std::{mem, ptr};
use winapi::shared::minwindef::DWORD;
use winapi::um::processthreadsapi::*;
use winapi::um::winnt::HANDLE;

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x00020016;
const PROC_THREAD_ATTRIBUTE_JOB_LIST: usize = 0x0002000D;

pub struct ProcThreadAttributeList {
    data: Vec<u8>,
    /// Backing storage for attributes whose value is a pointer to data.
    ///
    /// `UpdateProcThreadAttribute` only records such a pointer; it does not
    /// copy the data it points at, and that data must stay alive until
    /// `CreateProcess` returns.  Passing the address of a caller's temporary
    /// slice leaves a dangling pointer as soon as the call returns: any later
    /// stack or heap allocation may reuse that memory, and `CreateProcess`
    /// then reads a garbage handle (which surfaces as `ERROR_INVALID_HANDLE`).
    job_handles: Vec<HANDLE>,
}

impl ProcThreadAttributeList {
    pub fn with_capacity(num_attributes: DWORD) -> Result<Self, Error> {
        let mut bytes_required: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(
                ptr::null_mut(),
                num_attributes,
                0,
                &mut bytes_required,
            )
        };
        let mut data = Vec::with_capacity(bytes_required);
        // We have the right capacity, so force the vec to consider itself
        // that length.  The contents of those bytes will be maintained
        // by the win32 apis used in this impl.
        unsafe { data.set_len(bytes_required) };

        let attr_ptr = data.as_mut_slice().as_mut_ptr() as *mut _;
        let res = unsafe {
            InitializeProcThreadAttributeList(attr_ptr, num_attributes, 0, &mut bytes_required)
        };
        ensure!(
            res != 0,
            "InitializeProcThreadAttributeList failed: {}",
            IoError::last_os_error()
        );
        Ok(Self {
            data,
            job_handles: Vec::new(),
        })
    }

    pub fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.data.as_mut_slice().as_mut_ptr() as *mut _
    }

    pub fn set_pty(&mut self, con: HPCON) -> Result<(), Error> {
        let res = unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                con,
                mem::size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        ensure!(
            res != 0,
            "UpdateProcThreadAttribute failed: {}",
            IoError::last_os_error()
        );
        Ok(())
    }

    /// Put the child into these job objects at creation time.
    ///
    /// Takes a list because the attribute is defined as an array of job
    /// handles; nesting order is outermost first.  The array is copied into
    /// this list so that it outlives the call (see `job_handles`).
    pub fn set_job_list(&mut self, jobs: &[HANDLE]) -> Result<(), Error> {
        self.job_handles.clear();
        self.job_handles.extend_from_slice(jobs);
        let ptr = self.job_handles.as_ptr();
        let size = mem::size_of::<HANDLE>() * self.job_handles.len();
        let res = unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST,
                ptr as *mut _,
                size,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        ensure!(
            res != 0,
            "UpdateProcThreadAttribute(job) failed: {}",
            IoError::last_os_error()
        );
        Ok(())
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
    }
}

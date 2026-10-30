//! Thin OS layer: CPU affinity and page size.

/// CPUs this process may run on, in ascending order. Empty if unknown.
#[cfg(target_os = "linux")]
pub fn allowed_cpus() -> Vec<usize> {
    // SAFETY: cpu_set_t is plain data; sched_getaffinity fills it for the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return Vec::new();
        }
        (0..libc::CPU_SETSIZE as usize).filter(|&i| libc::CPU_ISSET(i, &set)).collect()
    }
}

/// Pin the calling thread to one CPU. Returns whether the OS accepted it.
#[cfg(target_os = "linux")]
pub fn pin_current_thread(cpu: usize) -> bool {
    if cpu >= libc::CPU_SETSIZE as usize {
        return false;
    }
    // SAFETY: as above; pid 0 means the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0
    }
}

#[cfg(target_os = "linux")]
pub fn page_size() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 { v as u64 } else { 0 }
}

#[cfg(not(target_os = "linux"))]
pub fn allowed_cpus() -> Vec<usize> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn pin_current_thread(_cpu: usize) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn page_size() -> u64 {
    0
}

/// Parse a kernel CPU list such as "0-3,8,10-11" into CPU numbers.
pub fn parse_cpu_list(s: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in s.trim().split(',').filter(|p| !p.trim().is_empty()) {
        let part = part.trim();
        let bad = || format!("invalid CPU list element '{part}'");
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
                if a > b {
                    return Err(bad());
                }
                out.extend(a..=b);
            }
            None => out.push(part.parse().map_err(|_| bad())?),
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_list_parsing() {
        assert_eq!(parse_cpu_list("0-3,8,10-11\n").unwrap(), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(parse_cpu_list("5,5,2").unwrap(), vec![2, 5]);
        assert!(parse_cpu_list("").unwrap().is_empty());
        assert!(parse_cpu_list("3-1").is_err());
        assert!(parse_cpu_list("a").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn can_query_and_pin() {
        let cpus = allowed_cpus();
        assert!(!cpus.is_empty());
        assert!(page_size() >= 4096);
        // Pin a throwaway thread so the test runner's own thread is untouched.
        let first = cpus[0];
        assert!(std::thread::spawn(move || pin_current_thread(first)).join().unwrap());
    }
}

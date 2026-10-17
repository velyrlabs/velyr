//! `node:os` APIs behind the `SysInfo` capability: denied by default, allowed per API name.

mod common;

use common::script::{CHECKS, assert_ok, run_with};
use velyr_runtime_permissions::{Capability, CapabilitySet, NamePattern};

const GATED: [&str; 3] = ["networkInterfaces", "userInfo", "loadavg"];

fn run_granting(names: &[&str], body: &str) -> String {
    let mut builder = CapabilitySet::builder();
    for name in names {
        builder = builder.allow(Capability::SysInfo(NamePattern::new(*name)));
    }
    let body =
        format!("{CHECKS}const os = globalThis[Symbol.for('velyr.node')].require('os');\n{body}");
    run_with(&body, builder.build(), "")
}

#[test]
fn gated_apis_throw_permission_denied_without_a_grant() {
    for api in GATED {
        assert_ok(run_granting(
            &[],
            &format!(
                "try {{ os.{api}(); throw new Error('no throw'); }} catch (e) {{ eq(e.code, 'VELYR_PERMISSION_DENIED', '{api}'); }}"
            ),
        ));
    }
}

#[test]
fn free_apis_work_without_a_grant() {
    assert_ok(run_granting(
        &[],
        "if (os.hostname().length === 0 || os.homedir().length === 0) throw new Error('free');
         if (os.cpus().length < 1) throw new Error('cpus');
         eq(typeof os.platform(), 'string'); eq(typeof os.arch(), 'string'); eq(os.EOL, '\\n');",
    ));
}

#[test]
fn grant_is_per_api_name() {
    assert_ok(run_granting(
        &["loadavg"],
        "eq(os.loadavg().length, 3);
         try { os.userInfo(); throw new Error('no throw'); } catch (e) { eq(e.code, 'VELYR_PERMISSION_DENIED'); }",
    ));
}

#[test]
fn user_info_with_grant() {
    assert_ok(run_granting(
        &["userInfo"],
        "const info = os.userInfo();
         if (info.username.length === 0 || info.shell !== null) throw new Error(JSON.stringify(info));",
    ));
}

#[test]
fn network_interfaces_lists_the_loopback() {
    assert_ok(run_granting(
        &["networkInterfaces"],
        "const nets = os.networkInterfaces();
         const lo = nets.lo0 || nets.lo;
         if (!lo) throw new Error('no loopback in ' + Object.keys(nets));
         const v4 = lo.find((i) => i.address === '127.0.0.1');
         if (!v4) throw new Error('no 127.0.0.1'); eq(v4.internal, true); eq(v4.netmask, '255.0.0.0');
         if (!/^([0-9a-f]{2}:){5}[0-9a-f]{2}$/.test(v4.mac)) throw new Error('mac ' + v4.mac);
         eq(v4.cidr, '127.0.0.1/8');",
    ));
}

#[test]
fn network_interfaces_masks_follow_node_format() {
    assert_ok(run_granting(
        &["networkInterfaces"],
        "for (const list of Object.values(os.networkInterfaces())) for (const i of list) {
           const prefix = Number(i.cidr.split('/')[1]);
           if (i.family === 'IPv4' && prefix === 8) eq(i.netmask, '255.0.0.0');
           if (i.family === 'IPv6' && prefix === 128) eq(i.netmask, 'ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff');
           if (i.family === 'IPv6' && prefix === 64) eq(i.netmask, 'ffff:ffff:ffff:ffff::');
           if (i.family === 'IPv6') eq(i.scopeid, 0);
         }",
    ));
}

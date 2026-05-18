// Spike pressure-test: a slice of a release workflow.
//
//   azdo run examples/release.rhai            # real
//   azdo run examples/release.rhai --dry-run  # reads run; writes stubbed
//
// Surface exercised: task() read + field access, string interpolation,
// conditional fail(), the DRY_RUN constant, build_start() with a params
// map. Deliberately small — this validates the surface, it is not a real
// release flow.

let id = 12345;

let t = task(id);
print(`task #${t.id} [${t.state}] ${t.title}`);

if t.state != "Ready for Release" {
    fail(`#${id} is "${t.state}", expected "Ready for Release"`);
}

if DRY_RUN {
    print("dry-run: the build below is stubbed");
}

let build = build_start("declaration", #{ "WorkItemId": "12345" });
print(`queued build #${build}`);

// daily — create today's standup wiki page, seeded with the content of the
// most recent existing one. The tool only creates the page; the final edit
// and save happen in the browser afterwards. Re-running on a day whose page
// already exists refuses to overwrite it.
//
// The gnarly bits (today's date, the YYYY/Q<n>/dd.MM.yyyy path, "which page
// is the latest") live in the host and are property-tested; this script is
// only the workflow *shape*. DAILY_ROOT comes from the [wiki] config.

let root = DAILY_ROOT;

let pages  = wiki_list(root);
let target = daily_target_path(root);

if pages.contains(target) {
    stop(`today's page already exists: ${target} — nothing to do`);
}

let source = pick_latest(pages);
if type_of(source) == "()" {
    fail(`no existing daily page under ${root} to copy from`);
}

print(`copying ${source} -> ${target}`);
let body = wiki_read(source);

if DRY_RUN {
    print(`[dry-run] would create ${target} (${body.len()} chars) from ${source}`);
} else {
    wiki_create(target, body);
    print(`created ${target} from ${source}. Now edit and save it in the browser.`);
}

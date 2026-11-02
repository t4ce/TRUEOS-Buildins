#!/usr/bin/env node
// Development-only oracle. Execute unchanged vendored jQuery algorithms;
// no npm packages, browser, network, clock, or upstream edits are required.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
globalThis.window = { document: {} };
const sourcePath = relative => path.join(root, "jquery-main", "src", relative);
const source = relative => fs.readFileSync(sourcePath(relative), "utf8");
const load = relative => import(pathToFileURL(sourcePath(relative)));
const { jQuery: $ } = await load("core.js");
const { cssCamelCase } = await load("css/cssCamelCase.js");
const { isAutoPx } = await load("css/isAutoPx.js");
const { rcssNum } = await load("var/rcssNum.js");
const { adjustCSS } = await load("css/adjustCSS.js");

// Tween imports the DOM CSS module, but its numeric run/easing definitions do
// not need that module. Remove import/export declarations only; setters are
// supplied explicitly and never access a DOM object or computed styles.
const tweenBody = source("effects/Tween.js")
    .replace(/^import\s+[^;]+;\s*/gm, "")
    .replace(/^export\s*\{[^}]*\};?\s*/gm, "");
new Function("jQuery", "isAutoPx", "finalPropName", tweenBody)($, isAutoPx, value => value);
$.Tween.propHooks.__oracle__ = { set() {} };

// effects.js itself probes the browser at module startup. Extract entire
// original definitions with checked lexical anchors; do not rewrite their
// algorithm bodies into the oracle.
const effectsSource = source("effects.js");
function block(startAnchor, endAnchor) {
    const start = effectsSource.indexOf(startAnchor);
    const end = effectsSource.indexOf(endAnchor, start);
    if (start < 0 || end < 0) throw Error(`Upstream effects extraction changed: ${startAnchor}`);
    return effectsSource.slice(start, end);
}
new Function("jQuery", block("jQuery.speed = function( speed, easing, fn ) {", "\njQuery.fn.extend("))($);
const speedsStart = effectsSource.indexOf("jQuery.fx.speeds = {");
const speedsEnd = effectsSource.indexOf("\n};", speedsStart);
if (speedsStart < 0 || speedsEnd < 0) throw Error("Upstream fx.speeds extraction changed");
new Function("jQuery", effectsSource.slice(speedsStart, speedsEnd + 3))($);
const tickBody = block("var currentTime = fxNow || createFxNow(),", "\n\n\t\t\tfor ( ; index < length;");
const progress = new Function("fxNow", "createFxNow", "animation", `${tickBody}\nreturn {percent, remaining_ms:remaining};`);

const cases = [];
const add = (op, input, result) => cases.push({ op, input: encode(input), result: encode(result) });
for (const property of ["", "-ms-transform", "-ms-grid-row", "-webkit-transform", "padding-left", "--custom-name", "foo-42", "a--b", "a-É", "é-A"])
    add("css_camel", property, cssCamelCase(property));

const properties = ["", "top", "right", "bottom", "left", "width", "height", "minWidth", "minHeight", "maxWidth", "maxHeight"];
for (const side of ["", "Top", "Right", "Bottom", "Left"]) {
    properties.push(`border${side}`, `border${side}Width`, `margin${side}`, `padding${side}`);
}
properties.push("opacity", "fontSize", "zIndex", "lineHeight", "borderRadius", "borderColor", "paddingWidth", "inset", "--width", "border-top-width");
for (const property of properties) {
    for (const variant of [property, property[0]?.toUpperCase() + property.slice(1), property.toLowerCase(), `${property}X`])
        add("auto_px", variant, isAutoPx(variant));
}

for (const operator of ["", "+=", "-=", "++="]) {
    for (const number of ["0", "-0", "+1", "001", "1.5", ".5", "-.5", "1.", "1e", "1e2", "1e-2", "1e+", "1e400", "Infinity", "NaN"]) {
        for (const unit of ["", "px", "em", "%", "PX", "%%", "px2", "π", " "]) {
            const input = operator + number + unit;
            const parts = rcssNum.exec(input);
            add("css_number", input, parts ? { relative: parts[1] ?? null, number: +parts[2], number_text: parts[2], unit: parts[3] } : null);
        }
    }
}

for (const property of ["width", "paddingLeft", "opacity"]) {
    for (const initial of [0, 1, -2, NaN]) {
        for (const target of ["+=9", "-=9", "-=-9px", "+=-.5em", "2e1px"]) {
            $.css = () => initial;
            add("endpoint", { property, initial, target }, adjustCSS({}, property, rcssNum.exec(target)));
        }
    }
}

const speeds = [undefined, null, false, true, 0, 100, -1, NaN, Infinity, "slow", "fast", "_default", "unknown",
    { duration: "fast", easing: "linear", queue: null, extra: true },
    { duration: 0, queue: false, complete: null },
    { duration: "slow", queue: "custom", old: "overwritten", special: 3 }, []];
for (const speed of speeds) {
    for (const off of [false, true]) {
        $.fx.off = off;
        const options = $.speed(speed);
        add("speed", { speed, off }, normalizedSpeed(options));
    }
}
for (const queue of [undefined, null, true, false, "fx", "custom", "", 0]) {
    $.fx.off = false;
    add("queue", queue, $.speed({ queue }).queue);
}
for (const easing of [undefined, false, null, "linear"]) {
    for (const complete of [undefined, false, "callback-token"]) {
        add("speed_args", { speed: 123, easing, complete }, normalizedSpeed($.speed(123, easing, complete)));
    }
}

for (const duration of [0, 100, -1, NaN]) {
    for (const easing of ["linear", "swing"]) {
        for (const percent of [0, 0.1, 0.25, 0.5, 0.75, 0.9, 1]) {
            for (const [start, end] of [[0, 1], [10, 30], [100, -50]]) {
                const tween = { start, end, now: start, options: { duration }, easing, prop: "__oracle__", elem: {} };
                $.Tween.prototype.run.call(tween, percent);
                add("tween", { duration, easing, percent, start, end }, { now: tween.now, pos: tween.pos });
            }
        }
    }
}
for (let index = 0; index <= 100; index++) {
    const percent = index / 100;
    add("swing", percent, $.easing.swing(percent));
}
for (const duration of [0, 1, 100, -1, NaN, Infinity]) {
    for (const now of [0, 100, 125, 200, 500]) {
        const animation = { startTime: 100, duration, tweens: [] };
        add("progress", { start: 100, duration, now }, progress(now, () => now, animation));
    }
}

function normalizedSpeed(options) {
    return {
        duration_ms: options.duration,
        easing: options.easing,
        queue: options.queue,
        completion: options.old,
        extra_options: Object.entries(options).filter(([name]) => !["duration", "easing", "queue", "complete", "old"].includes(name))
    };
}
function encode(value) {
    if (typeof value === "number" && !Number.isFinite(value)) return { $number: `${value}` };
    if (value === undefined) return { $undefined: true };
    if (Array.isArray(value)) return value.map(encode);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, encode(child)]));
    return value;
}

const output = JSON.stringify({ description: "Original vendored jQuery CSS/effects results; see tools/style_oracle.mjs", cases }, null, 2) + "\n";
const destination = path.join(root, "tests/fixtures/style.json");
if (process.argv.includes("--write")) {
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, output);
    console.log(`Recorded ${cases.length} upstream CSS/effects cases`);
} else {
    if (fs.readFileSync(destination, "utf8") !== output) throw Error("CSS/effects oracle differs: inspect source changes before --write");
    console.log(`Verified ${cases.length} upstream CSS/effects cases`);
}

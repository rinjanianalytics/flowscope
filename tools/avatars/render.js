// Render every part in parts.js to a transparent 160x160 PNG in
// crates/flowscope-console/data/avatars (<part>_<nn>.png) and rewrite partlist.txt.
// Run: npm i --no-save playwright && node tools/avatars/render.js
// Then bump AVATAR_VERSION in crates/flowscope-console/src/auth.rs.
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');
const P = require('./parts.js');

const out = path.resolve(__dirname, '../../crates/flowscope-console/data/avatars');
fs.mkdirSync(out, { recursive: true });
for (const f of fs.readdirSync(out)) if (/^[a-z]+_\d+\.png$/.test(f)) fs.rmSync(path.join(out, f));
const svg = (body) => `<svg xmlns="http://www.w3.org/2000/svg" width="160" height="160" viewBox="0 0 160 160">${body}</svg>`;

const sets = [
    ['background', P.backgrounds],
    ['body', P.bodies.map((b) => b[1])],
    ['eyes', P.eyes],
    ['mouth', P.mouths],
    ['accessory', P.accessories],
];

(async () => {
    const b = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
    const page = await b.newPage({ viewport: { width: 160, height: 160 }, deviceScaleFactor: 1 });
    for (const [name, list] of sets) {
        for (let i = 0; i < list.length; i++) {
            const file = path.join(out, `${name}_${String(i + 1).padStart(2, '0')}.png`);
            await page.setContent(`<html><body style="margin:0;background:transparent">${svg(list[i])}</body></html>`);
            await page.locator('svg').screenshot({ path: file, omitBackground: true });
            
        }
    }
    fs.writeFileSync(path.join(out, 'partlist.txt'), sets.map((s) => s[0]).join('\n') + '\n');
    await b.close();
    console.log(sets.map(([n, l]) => `${n} ${l.length}`).join(', '));
})();

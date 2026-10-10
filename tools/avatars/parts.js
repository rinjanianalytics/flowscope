// FlowScope avatar parts: little network "bots" in the Rinjani palette.
// Every part is a full 160x160 layer; layers stack in partlist order.
const C = {
    teal: '#35776F', mid: '#509087', sage: '#789B78', slate: '#5A6A66', forest: '#253834',
    plate: '#F4F5F7', ink: '#1B2624', white: '#FFFFFF',
};
const shade = (hex, k) => '#' + [1, 3, 5].map((i) => Math.round(parseInt(hex.slice(i, i + 2), 16) * k).toString(16).padStart(2, '0')).join('');

// Sticker outline so an accessory reads on any body colour.
const st = 'stroke="#FFFFFF" stroke-width="6" stroke-linejoin="round" paint-order="stroke"';

const backgrounds = [
    ['#E3EFEC', '#35776F'], ['#D6E8E4', '#35776F'], ['#E8EEE5', '#789B78'],
    ['#E5E9E8', '#5A6A66'], ['#EEF1F3', '#509087'], ['#DDE9E1', '#509087'],
].map(([bg, tint]) => `
  <rect width="160" height="160" fill="${bg}"/>
  <path d="M0 118 C34 96 70 138 108 118 S150 98 160 104 V160 H0 Z" fill="${tint}" fill-opacity="0.10"/>
  <path d="M0 136 C40 118 76 152 116 134 S152 122 160 126 V160 H0 Z" fill="${tint}" fill-opacity="0.14"/>`);

// Heads share one box (x 30..130, top near y 36) so hats, headsets and the face line up on all of them.
const heads = {
    block: (c) => `<rect x="30" y="36" width="100" height="92" rx="26" fill="${c}"/>`,
    puck: (c) => `<rect x="30" y="36" width="100" height="92" rx="46" fill="${c}"/>`,
    rack: (c) => `<rect x="24" y="58" width="12" height="48" rx="4" fill="${shade(c, 0.8)}"/><rect x="124" y="58" width="12" height="48" rx="4" fill="${shade(c, 0.8)}"/>
      <circle cx="30" cy="66" r="2" fill="${C.plate}" fill-opacity="0.7"/><circle cx="30" cy="98" r="2" fill="${C.plate}" fill-opacity="0.7"/>
      <circle cx="130" cy="66" r="2" fill="${C.plate}" fill-opacity="0.7"/><circle cx="130" cy="98" r="2" fill="${C.plate}" fill-opacity="0.7"/>
      <rect x="32" y="38" width="96" height="88" rx="14" fill="${c}"/>`,
    hex: (c) => `<path d="M80 34 L126 57 V107 L80 130 L34 107 V57 Z" fill="${c}" stroke="${c}" stroke-width="10" stroke-linejoin="round"/>`,
    shield: (c) => `<path d="M80 32 L130 46 V82 C130 110 108 126 80 134 C52 126 30 110 30 82 V46 Z" fill="${c}" stroke="${c}" stroke-width="6" stroke-linejoin="round"/>`,
    monitor: (c) => `<rect x="70" y="112" width="20" height="14" fill="${shade(c, 0.8)}"/><rect x="28" y="36" width="104" height="80" rx="14" fill="${c}"/>`,
};
const bodyColours = ['teal', 'mid', 'sage', 'slate', 'forest'];
const bodies = [];
for (const [name, head] of Object.entries(heads)) for (const col of bodyColours) {
    const c = C[col];
    const leds = name === 'monitor' ? '' : `<circle cx="104" cy="117" r="2.5" fill="${C.plate}" fill-opacity="0.75"/><circle cx="112" cy="117" r="2.5" fill="${C.plate}" fill-opacity="0.45"/>`;
    bodies.push([`${name}-${col}`, `
  <rect x="34" y="122" width="92" height="60" rx="30" fill="${shade(c, 0.78)}"/>
  ${head(c)}
  <rect x="48" y="58" width="64" height="50" rx="16" fill="${C.plate}"/>
  ${leds}`]);
}

const I = C.ink;
const L = 66, R = 94, Y = 78; // eye centres
const eyes = [
    `<circle cx="${L}" cy="${Y}" r="5.5" fill="${I}"/><circle cx="${R}" cy="${Y}" r="5.5" fill="${I}"/>`,
    `<ellipse cx="${L}" cy="${Y}" rx="4.5" ry="7" fill="${I}"/><ellipse cx="${R}" cy="${Y}" rx="4.5" ry="7" fill="${I}"/>`,
    `<path d="M59 81 Q66 71 73 81 M87 81 Q94 71 101 81" fill="none" stroke="${I}" stroke-width="4" stroke-linecap="round"/>`,
    `<circle cx="${L}" cy="${Y}" r="5.5" fill="${I}"/><path d="M87 80 Q94 72 101 80" fill="none" stroke="${I}" stroke-width="4" stroke-linecap="round"/>`,
    `<rect x="61" y="73" width="10" height="10" rx="2" fill="${I}"/><rect x="89" y="73" width="10" height="10" rx="2" fill="${I}"/>`,
    `<circle cx="${L}" cy="${Y}" r="6.5" fill="${I}"/><circle cx="${R}" cy="${Y}" r="6.5" fill="${I}"/><circle cx="68.5" cy="75.5" r="2.2" fill="${C.white}"/><circle cx="96.5" cy="75.5" r="2.2" fill="${C.white}"/>`,
    `<path d="M59.5 77 A6.5 6.5 0 0 0 72.5 77 Z M87.5 77 A6.5 6.5 0 0 0 100.5 77 Z" fill="${I}"/><path d="M58 77 H74 M86 77 H102" stroke="${I}" stroke-width="3" stroke-linecap="round"/>`,
    `<circle cx="${L}" cy="${Y}" r="7.5" fill="${C.white}" stroke="${I}" stroke-width="2.5"/><circle cx="${R}" cy="${Y}" r="7.5" fill="${C.white}" stroke="${I}" stroke-width="2.5"/><circle cx="69" cy="${Y}" r="3.5" fill="${I}"/><circle cx="97" cy="${Y}" r="3.5" fill="${I}"/>`,
    `<rect x="59" y="75.5" width="14" height="5.5" rx="2.75" fill="${C.teal}"/><rect x="87" y="75.5" width="14" height="5.5" rx="2.75" fill="${C.teal}"/>`,
    `<circle cx="${L}" cy="80" r="5" fill="${I}"/><circle cx="${R}" cy="80" r="5" fill="${I}"/><path d="M59 70 L72 73 M101 70 L88 73" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<path d="M61 72.5 L71 78 L61 83.5 M99 72.5 L89 78 L99 83.5" fill="none" stroke="${I}" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/>`,
];

const mouths = [
    `<path d="M72 94 Q80 102 88 94" fill="none" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<circle cx="80" cy="97" r="4" fill="${I}"/>`,
    `<path d="M73 97 H87" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<path d="M70 92 H90 Q90 104 80 104 Q70 104 70 92 Z" fill="${I}" stroke="${I}" stroke-width="2" stroke-linejoin="round"/><ellipse cx="80" cy="101" rx="5" ry="2.6" fill="${C.mid}"/>`,
    `<path d="M73 98 Q82 101 88 93" fill="none" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<path d="M67 97 q3.25 -6 6.5 0 t6.5 0 t6.5 0 t6.5 0" fill="none" stroke="${C.teal}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>`,
    `<path d="M71 94 q4.5 6 9 0 q4.5 6 9 0" fill="none" stroke="${I}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>`,
    `<rect x="69" y="91" width="22" height="11" rx="4" fill="${I}"/><rect x="72" y="93" width="16" height="3.5" rx="1" fill="${C.white}"/>`,
    `<path d="M88 100 v3 a4 4 0 0 1 -8 0 v-3 Z" fill="${C.mid}"/><path d="M71 95 Q80 103 89 95" fill="none" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<ellipse cx="58" cy="92" rx="5" ry="3" fill="${C.sage}" fill-opacity="0.45"/><ellipse cx="102" cy="92" rx="5" ry="3" fill="${C.sage}" fill-opacity="0.45"/><path d="M74 94 Q80 100 86 94" fill="none" stroke="${I}" stroke-width="3.5" stroke-linecap="round"/>`,
    `<ellipse cx="80" cy="97" rx="5" ry="7" fill="${I}"/>`,
];

const F = C.forest;
const accessories = [
    '', '', '',
    // antenna with signal
    `<path d="M80 38 V18" stroke="${C.white}" stroke-width="10" stroke-linecap="round"/>
     <path d="M80 38 V18" stroke="${F}" stroke-width="4" stroke-linecap="round"/>
     <circle cx="80" cy="14" r="6.5" fill="${C.teal}" ${st}/>
     <path d="M91 9 a10 10 0 0 1 0 12 M97 5 a16 16 0 0 1 0 20" fill="none" stroke="${C.teal}" stroke-width="3" stroke-linecap="round"/>`,
    // headset
    `<path d="M30 74 C30 26 130 26 130 74" fill="none" stroke="${C.white}" stroke-width="13" stroke-linecap="round"/>
     <path d="M30 74 C30 26 130 26 130 74" fill="none" stroke="${F}" stroke-width="7" stroke-linecap="round"/>
     <rect x="20" y="64" width="16" height="32" rx="7" fill="${F}" ${st}/><rect x="124" y="64" width="16" height="32" rx="7" fill="${F}" ${st}/>
     <path d="M28 94 Q34 112 60 112" fill="none" stroke="${C.white}" stroke-width="8" stroke-linecap="round"/>
     <path d="M28 94 Q34 112 60 112" fill="none" stroke="${F}" stroke-width="3.5" stroke-linecap="round"/>
     <circle cx="62" cy="112" r="4.5" fill="${C.teal}" ${st}/>`,
    // glasses
    `<path d="M44 76 H55 M105 76 H116" stroke="${F}" stroke-width="3" stroke-linecap="round"/>
     <rect x="55" y="67" width="22" height="20" rx="7" fill="${C.white}" fill-opacity="0.18" stroke="${F}" stroke-width="3.5"/>
     <rect x="83" y="67" width="22" height="20" rx="7" fill="${C.white}" fill-opacity="0.18" stroke="${F}" stroke-width="3.5"/>
     <path d="M77 76 Q80 72 83 76" fill="none" stroke="${F}" stroke-width="3"/>`,
    // sunglasses
    `<path d="M44 74 H116" stroke="${F}" stroke-width="3.5" stroke-linecap="round"/>
     <path d="M54 70 H78 V78 Q78 88 66 88 Q54 88 54 78 Z M82 70 H106 V78 Q106 88 94 88 Q82 88 82 78 Z" fill="${F}"/>
     <path d="M59 74 L64 74 M87 74 L92 74" stroke="${C.mid}" stroke-width="2.5" stroke-linecap="round"/>`,
    // cap
    `<path d="M33 52 C33 20 127 20 127 52 Z" fill="${F}" ${st}/>
     <path d="M96 52 H142 Q146 52 144 57 Q142 60 136 60 H96 Z" fill="${C.teal}" ${st}/>
     <path d="M33 52 C33 20 127 20 127 52 Z" fill="${F}"/>
     <path d="M80 23 V52" stroke="${C.teal}" stroke-width="2" stroke-opacity="0.8"/>
     <circle cx="80" cy="23" r="3.5" fill="${C.teal}"/>`,
    // Rinjani peak beanie
    `<path d="M30 54 L56 26 L68 36 L86 14 L130 54 Z" fill="${C.teal}" ${st}/>
     <path d="M78 24 L86 14 L95 22 L90 26 L86 22 L82 27 Z" fill="${C.white}"/>
     <rect x="28" y="48" width="104" height="10" rx="5" fill="${F}" ${st}/>`,
    // sprout
    `<path d="M80 38 V22" stroke="${C.white}" stroke-width="8" stroke-linecap="round"/>
     <path d="M80 38 V22" stroke="${F}" stroke-width="3.5" stroke-linecap="round"/>
     <path d="M80 24 C80 12 68 6 58 10 C60 20 70 26 80 24 Z" fill="${C.sage}" ${st}/>
     <path d="M80 22 C80 10 92 4 102 8 C100 18 90 24 80 22 Z" fill="${C.mid}" ${st}/>`,
    // bow tie
    `<path d="M80 136 L62 126 V146 Z M80 136 L98 126 V146 Z" fill="${C.plate}" stroke-linejoin="round" stroke="${C.plate}" stroke-width="3"/>
     <rect x="75" y="131" width="10" height="10" rx="3" fill="${C.teal}"/>`,
    // lanyard badge
    `<path d="M60 124 L76 146 M100 124 L84 146" stroke="${C.mid}" stroke-width="3.5" stroke-linecap="round"/>
     <rect x="68" y="142" width="24" height="16" rx="3" fill="${C.plate}"/>
     <rect x="68" y="142" width="24" height="5" rx="2" fill="${C.teal}"/>
     <path d="M72 152 H84" stroke="${C.slate}" stroke-width="2" stroke-linecap="round"/>`,
    // party hat
    `<path d="M62 42 L80 10 L98 42 Z" fill="${C.mid}" ${st}/>
     <path d="M69 30 L91 30 M65 37 L95 37 M74 21 L86 21" stroke="${C.white}" stroke-width="2.5" stroke-linecap="round" stroke-opacity="0.85"/>
     <circle cx="80" cy="9" r="5" fill="${C.sage}" ${st}/>`,
];

module.exports = { backgrounds, bodies, eyes, mouths, accessories, C };

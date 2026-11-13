// Comparação ampla de regex com o Node (saída linha a linha).
function show(label, f) {
  let r;
  try { r = f(); } catch (e) { r = e.name + ': ' + e.message; }
  console.log(label + ' => ' + JSON.stringify(r));
}
const textos = [
  'Olá Mundo! ação ÇÃO straße STRASSE ǅ ǆ Ǆ Kelvin K k ſ s S',
  'Ελληνικά ΣΊΣΥΦΟΣ σίσυφος русский ЁЖИК ёжик 日本語 ひらがな カタカナ 한국어 中文',
  'emoji 👍🏽 🇧🇷 👨‍👩‍👧 #️⃣ 🏴󠁧󠁢󠁳󠁣󠁴󠁿 ❤️ ✨ 1️⃣',
  'mix 𝒜𝒷𝒸 𐐀𐐨 𞤀𞤢 ﬀ ﬃ ŉ ǰ ΐ ẞ ß',
  'linhas\nduas\r\ntrês quatro',
];
const padroes = [
  ['\\p{L}+', 'gu'], ['\\p{Lu}', 'gu'], ['\\p{Ll}', 'giu'], ['\\P{Ll}+', 'giu'], ['\\P{Ll}+', 'giv'],
  ['\\p{sc=Greek}+', 'gu'], ['\\p{scx=Hira}+', 'gu'], ['\\p{Script=Han}+', 'gu'], ['\\p{Emoji_Presentation}', 'gu'],
  ['\\p{Extended_Pictographic}', 'gu'], ['\\p{RI}{2}', 'gu'], ['\\p{EMod}', 'gu'], ['\\p{Nd}+', 'gu'], ['\\p{P}+', 'gu'],
  ['\\p{RGI_Emoji}', 'gv'], ['\\p{Basic_Emoji}', 'gv'], ['[\\p{RGI_Emoji}--\\p{Basic_Emoji}]', 'gv'],
  ['[\\p{L}&&\\p{sc=Latn}]+', 'gv'], ['[\\w--\\d]+', 'gv'], ['[\\q{straße|ss}]', 'giv'], ['[^\\p{L}\\s]+', 'gv'],
  ['k', 'gi'], ['k', 'giu'], ['s', 'giu'], ['ß', 'giu'], ['ẞ', 'gi'], ['σ', 'gi'], ['Σ', 'giu'], ['ς', 'giu'], ['ǅ', 'gi'],
  ['ǆ', 'giu'], ['ﬀ', 'giu'], ['ΐ', 'giu'], ['𐐀', 'giu'], ['𐐀', 'gi'], ['\\u{1E900}', 'giu'],
  ['[a-z]+', 'gi'], ['[a-z]+', 'giu'], ['[^a-z\\s]+', 'gi'], ['[^a-z\\s]+', 'giu'], ['[^a-z\\s]+', 'giv'],
  ['\\w+', 'giu'], ['\\W+', 'giu'], ['\\b\\w', 'giu'], ['\\B.', 'giu'],
  ['(?i:k)elvin', 'g'], ['(?i:ç)ão', 'g'], ['(?-i:ç)ão', 'gi'], ['^\\w+$', 'gm'], ['(?m:^\\w+$)', 'g'], ['.+', 'g'], ['(?s:.+)', 'g'],
  ['(?<a>\\w)\\k<a>', 'g'], ['(?<v>[aeiou])|(?<v>[AEIOU])', 'g'], ['(?:(?<x>a)|(?<x>b))\\k<x>', 'gi'],
  ['(.)\\1', 'giu'], ['(?<=\\p{Lu})\\p{Ll}+', 'gu'], ['(?<!\\p{L})\\p{Nd}', 'gu'],
  ['\\u{1F44D}\\p{EMod}?', 'gu'], ['.', 'gu'], ['[^]', 'g'], ['\\S+', 'gv'],
];
for (const [p, f] of padroes) {
  for (let i = 0; i < textos.length; i++) {
    show(`/${p}/${f} #${i}`, () => {
      const re = new RegExp(p, f);
      const out = [];
      let m;
      while ((m = re.exec(textos[i])) !== null) {
        out.push([m.index, m[0], m.groups ? JSON.stringify(m.groups) : '']);
        if (m[0] === '') re.lastIndex++;
        if (out.length > 60) break;
      }
      return out;
    });
  }
}
// sintaxe e erros
const erros = [
  ['\\p{Foo}', 'u'], ['\\p{ascii}', 'u'], ['\\p{RGI_Emoji}', 'u'], ['[^\\p{RGI_Emoji}]', 'v'], ['[a-z&&b]', 'v'],
  ['(?ii:a)', ''], ['(?-:a)', ''], ['(?<a>.)(?<a>.)', ''], ['\\k<z>(?<a>.)', ''], ['[\\q{a|bc}]', 'u'], ['(?i-s:a)', ''],
  ['[\\p{Lu}--[A-Z]]', 'v'], ['\\p{Script_Extensions=Latin}', 'u'], ['\\p{scx=Zyyy}', 'u'], ['\\p{gc=LC}', 'u'],
];
for (const [p, f] of erros) show(`erro /${p}/${f}`, () => String(new RegExp(p, f)));
// métodos
show('split', () => 'a1b22c333'.split(/(\d+)/));
show('split u', () => '😀a😀'.split(/(?:)/u));
show('replace $<>', () => '2026-11-12'.replace(/(?<a>\d+)-(?<m>\d+)-(?<d>\d+)/, '$<d>/$<m>/$<a>'));
show('replaceAll', () => 'aXbXc'.replaceAll(/x/gi, (m, i) => i));
show('matchAll', () => [...'a1b2c3'.matchAll(/(?<l>[a-z])(?<n>\d)/g)].map(m => [m.index, m.groups.l, m.groups.n]));
show('search', () => 'abcDEF'.search(/d/i));
show('indices', () => /(?<x>b)(c)?/d.exec('abc').indices);
show('escape', () => RegExp.escape('a.b*c\n  -x'));
show('source', () => [new RegExp('/', 'g').source, new RegExp('\n').source, RegExp('a\\\nb').source, String(/[/]/)]);
show('flags', () => /a/dgimsvy.flags);
show('toString', () => String(new RegExp('(?i:a)b', 'gv')));
show('lastIndex u', () => { const r = /./gu; r.lastIndex = 1; const m = r.exec('😀x'); return [m.index, m[0], r.lastIndex]; });
show('sticky', () => { const r = /b/y; r.lastIndex = 1; return [r.test('abc'), r.lastIndex, r.test('abc'), r.lastIndex]; });
show('species split', () => { class R extends RegExp {} ; return 'a,b'.split(new R(',')); });
show('regexp-like', () => String(RegExp({ [Symbol.match]: true, source: 'x+', flags: 'g' })));

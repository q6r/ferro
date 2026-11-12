const re = /(\d{4})-(\d{2})-(\d{2})/;
console.log(re.exec("data: 2026-11-11 ok"), re.test("x"), re.source, re.flags, String(re));
const named = /(?<ano>\d{4})-(?<mes>\d{2})/u.exec("em 2024-03");
console.log(named.groups, named.groups.ano, named.index, named.length);
console.log("a1b22c333".match(/\d+/g), "abc".match(/x/g), "abc".match(/(b)(c)?/), "aaa".match(/a*?/));
console.log([..."x1y22z333".matchAll(/[a-z](\d+)/g)].map(m => [m[0], m[1], m.index]));
console.log("2026-11-11".replace(/(\d+)-(\d+)-(\d+)/, "$3/$2/$1"), "abc".replace(/b/, "[$&|$`|$']"), "aaa".replace(/a/g, "$$"));
console.log("John Smith".replace(/(?<first>\w+)\s(?<last>\w+)/, "$<last>, $<first>"), "x".replace(/x/, "$1$0$10"));
console.log("hello world".replace(/o/g, (m, off) => `<${m}${off}>`), "a-b_c d".replace(/[-_ ](\w)/g, (_, c) => c.toUpperCase()));
console.log("camelCaseString".replace(/([A-Z])/g, " $1").toLowerCase(), "  muito   espaço  ".replace(/\s+/g, " ").trim());
console.log("a,b;c d".split(/[,; ]/), "a1b2c3".split(/\d/), "abc".split(/(b)/), "a,b,c".split(/,/, 2), "".split(/,/), "abc".split(/(?:)/));
console.log("Olá Mundo".search(/mundo/i), "abc".search(/z/), /^[\w.+-]+@\w+\.\w{2,}$/.test("ana.b+1@site.com"), /^\p{Lu}/u.test("Ágata"));
const g = /o/g; console.log(g.test("foo"), g.lastIndex, g.test("foo"), g.lastIndex, g.test("foo"), g.lastIndex);
const y = /a/y; y.lastIndex = 1; console.log(y.test("ba"), y.lastIndex, y.test("ba"), y.lastIndex);
console.log(/(?<=\$)\d+(\.\d\d)?/.exec("custa $42.50 hoje")[0], /\d+(?!%)\b/.exec("50% de 80")[0], /(?<!-)\b\d+/.exec("-5 e 7")[0]);
console.log(/^.$/u.test("😀"), /^.$/.test("😀"), "😀😁".match(/./gu), /\u{1F600}/u.test("😀"), /[😀]/u.test("😀"));
console.log(/a.c/s.test("a\nc"), /a.c/.test("a\nc"), /^b$/m.test("a\nb\nc"), /ABC/i.test("aBc"), /ß/i.test("SS"));
console.log(/(a)|b/.exec("b"), /(?:(a)|b)+/.exec("ab"), /(\w)\1/.exec("hello"), /(?<l>\w)\k<l>/.exec("aabb").groups);
console.log(new RegExp("a/b").source, String(new RegExp("\n")), new RegExp("").source, String(new RegExp(/x/g, "i")), RegExp("a", "g").global);
for (const p of ["(", "[a", "a**", "?", "(?<n>a)(?<n>b)", "\\"]) { try { new RegExp(p); console.log("ok", p); } catch (e) { console.log(e.name, e.message); } }
try { new RegExp("a", "gg"); } catch (e) { console.log(e.name, e.message); }
console.log(/\bfoo\b/.test("a foo."), /\Bfoo/.test("afoo"), /[^\s]+/.exec("  hi  ")[0], /\D+/.exec("12ab3")[0], /\W/.exec("ab-c")[0]);
console.log("tel: (11) 98765-4321".replace(/\D/g, ""), "R$ 1.234,56".replace(/[^\d,]/g, "").replace(",", "."));
const url = /^(https?):\/\/([^/:]+)(?::(\d+))?(\/[^?#]*)?(?:\?([^#]*))?(?:#(.*))?$/;
console.log(url.exec("https://example.com:8080/a/b?x=1&y=2#topo").slice(1));
const tpl = "Olá {{nome}}, você tem {{n}} mensagens {{falta}}"; const dados = { nome: "Ana", n: 3 };
console.log(tpl.replace(/\{\{(\w+)\}\}/g, (m, k) => k in dados ? dados[k] : m));
console.log("a.b*c".replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), /x{2,}/.exec("axxxb")[0], /x{0}y/.exec("xy")[0], /a{2,3}?/.exec("aaaa")[0]);
console.log(/(a*)*b/.exec("aaab"), /(a|ab)(c|bcd)(d*)/.exec("abcd"), /((a)|b)+/.exec("ab"));
console.log(/(?=(\d+))\d/.exec("123"), /(?!a)\w/.exec("ab"), /(.)(?=\1)/.exec("abccd"));
console.log("aBc".replace(/b/i, "X"), "x-y-z".replaceAll(/-/g, "+"), "abc".replace(/(?<x>b)/, (...a) => JSON.stringify(a.at(-1))));
try { "x".replaceAll(/x/, "y"); } catch (e) { console.log(e.name, e.message); }
try { "x".matchAll(/x/); } catch (e) { console.log(e.name); }
const comD = /(?<n>b+)/d.exec("abbc"); console.log(comD.indices, comD.indices.groups.n);
console.log(Object.prototype.toString.call(/x/), /x/ instanceof RegExp, typeof /x/, /[/]/.source, /\//.source);
const linhas = "nome=Ana\nidade=30\n\ncidade = São Paulo"; const cfg = {};
for (const [, k, v] of linhas.matchAll(/^\s*(\w+)\s*=\s*(.*)$/gm)) cfg[k] = v; console.log(cfg);
console.log("The Quick Brown".match(/\b[A-Z]/g).join(""), "abc123def".replace(/\d+/, n => n * 2), "aaa".lastIndexOf("a"), /[À-ÿ]/.test("é"));
const csv = 'a,"b,c",d,"e ""f"""'; console.log(csv.match(/("([^"]|"")*"|[^,]*)(,|$)/g));
console.log("x".padEnd(3, "ab").replace(/b$/, "B"), /^(?:\+55\s?)?\(?\d{2}\)?\s?9?\d{4}-?\d{4}$/.test("+55 (11) 98765-4321"));

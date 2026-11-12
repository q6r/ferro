const n = 1234567.891;
for (const loc of ["en-US", "pt-BR", "de-DE", "es-ES", "fr-FR", "it-IT"]) {
  console.log(loc, new Intl.NumberFormat(loc).format(n), n.toLocaleString(loc, { maximumFractionDigits: 1 }), (0.256).toLocaleString(loc, { style: "percent" }), (1234).toLocaleString(loc));
}
const moeda = [["en-US", "USD"], ["pt-BR", "BRL"], ["pt-BR", "USD"], ["de-DE", "EUR"], ["en-US", "EUR"], ["en-US", "JPY"], ["fr-FR", "EUR"]];
for (const [loc, cur] of moeda) console.log(loc, cur, new Intl.NumberFormat(loc, { style: "currency", currency: cur }).format(-1234.5));
for (const v of [999, 1000, 1234, 15300, 999999, 1500000, 2340000000, 7.2e12]) {
  console.log(v, new Intl.NumberFormat("en-US", { notation: "compact" }).format(v), new Intl.NumberFormat("pt-BR", { notation: "compact" }).format(v), new Intl.NumberFormat("en", { notation: "compact", compactDisplay: "long" }).format(v));
}
console.log(new Intl.NumberFormat("en-US", { minimumFractionDigits: 2 }).format(5), new Intl.NumberFormat("en-US", { maximumSignificantDigits: 3 }).format(123456), new Intl.NumberFormat("en-US", { minimumIntegerDigits: 3 }).format(7));
console.log(new Intl.NumberFormat("en-US", { signDisplay: "always" }).format(5), new Intl.NumberFormat("en-US", { signDisplay: "exceptZero" }).format(0), new Intl.NumberFormat("en-US", { useGrouping: false }).format(12345));
console.log(new Intl.NumberFormat("en-US", { style: "unit", unit: "kilometer-per-hour" }).format(50), new Intl.NumberFormat("en-US", { style: "unit", unit: "megabyte" }).format(1.5));
console.log(JSON.stringify(new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).formatToParts(-1234.5)));
console.log([1, 2.5, 1000].map(new Intl.NumberFormat("pt-BR").format), new Intl.NumberFormat("pt-BR").resolvedOptions().locale);
const d = new Date(Date.UTC(2026, 10, 11, 18, 5, 9));
for (const loc of ["en-US", "pt-BR", "de-DE", "es-ES", "fr-FR", "en-GB"]) {
  const f = (o) => new Intl.DateTimeFormat(loc, { timeZone: "UTC", ...o }).format(d);
  console.log(loc, "|", f({}), "|", f({ dateStyle: "full" }), "|", f({ dateStyle: "long" }), "|", f({ dateStyle: "medium" }), "|", f({ dateStyle: "short" }));
  console.log(loc, "|", f({ timeStyle: "short" }), "|", f({ timeStyle: "medium" }), "|", f({ month: "long", year: "numeric" }), "|", f({ month: "short", day: "numeric" }), "|", f({ weekday: "long" }));
  console.log(loc, "|", f({ hour: "2-digit", minute: "2-digit" }), "|", f({ year: "numeric", month: "2-digit", day: "2-digit" }), "|", f({ dateStyle: "medium", timeStyle: "short" }));
}
console.log(d.toLocaleDateString("en-US", { timeZone: "UTC" }), d.toLocaleTimeString("en-US", { timeZone: "UTC" }), d.toLocaleString("pt-BR", { timeZone: "UTC" }), d.toLocaleDateString("pt-BR", { timeZone: "UTC", day: "numeric", month: "long" }));
console.log(JSON.stringify(new Intl.DateTimeFormat("en-US", { timeZone: "UTC", hour: "numeric", minute: "2-digit" }).formatToParts(d)));
const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" }), rtp = new Intl.RelativeTimeFormat("pt-BR", { numeric: "auto" }), rta = new Intl.RelativeTimeFormat("en");
console.log(rtf.format(-1, "day"), rtf.format(1, "day"), rtf.format(0, "day"), rtf.format(-3, "hours"), rtf.format(2, "week"), rta.format(-1, "day"), rta.format(1, "month"));
console.log(rtp.format(-1, "day"), rtp.format(1, "day"), rtp.format(-5, "minute"), rtp.format(3, "year"), rtp.format(-1, "month"), new Intl.RelativeTimeFormat("es").format(-2, "day"));
const pr = new Intl.PluralRules("en-US"), po = new Intl.PluralRules("en-US", { type: "ordinal" }), pp = new Intl.PluralRules("pt-BR");
console.log([0, 1, 2, 1.5].map(x => pr.select(x)), [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101].map(x => po.select(x)), [0, 1, 1.5, 2].map(x => pp.select(x)));
console.log(new Intl.ListFormat("en").format(["a", "b", "c"]), new Intl.ListFormat("pt-BR").format(["a", "b", "c"]), new Intl.ListFormat("en", { type: "disjunction" }).format(["a", "b"]), new Intl.ListFormat("es").format(["x", "y"]));
const nomes = ["Zé", "ana", "Álvaro", "bruno", "Ana", "éder", "item10", "item2"];
console.log([...nomes].sort(new Intl.Collator("pt-BR").compare), [...nomes].sort(new Intl.Collator("en", { numeric: true }).compare));
console.log("a".localeCompare("B"), "á".localeCompare("a"), "a".localeCompare("A"), "résumé".localeCompare("resume", "en", { sensitivity: "base" }), "10".localeCompare("9", undefined, { numeric: true }));
console.log([...new Intl.Segmenter("en", { granularity: "word" }).segment("Olá, mundo!")].filter(s => s.isWordLike).map(s => s.segment), [...new Intl.Segmenter().segment("é👍🏽a")].map(s => s.segment).length);
console.log(Intl.getCanonicalLocales(["PT-br", "EN-us"]), new Intl.Locale("pt-BR").language, new Intl.Locale("pt-BR").region, typeof Intl, Object.prototype.toString.call(new Intl.NumberFormat()));

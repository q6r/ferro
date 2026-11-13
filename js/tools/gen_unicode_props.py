"""Gera js/src/unicode_props.rs (tabelas Unicode do motor de regex do Ferro JS).

uso: python -I gen_unicode_props.py <pasta-do-ucd> <saida.rs>

A pasta tem os arquivos do Unicode Character Database 17.0.0, na estrutura
de https://www.unicode.org/Public/17.0.0/ucd/ (só dados, nada é executado):

  UnicodeData.txt  PropertyAliases.txt  PropertyValueAliases.txt
  Scripts.txt  ScriptExtensions.txt  PropList.txt  DerivedCoreProperties.txt
  DerivedNormalizationProps.txt  CaseFolding.txt  SpecialCasing.txt
  extracted/DerivedGeneralCategory.txt  extracted/DerivedBinaryProperties.txt
  emoji/emoji-data.txt

e mais, de https://www.unicode.org/Public/17.0.0/emoji/, numa subpasta
`emoji-seq`: emoji-sequences.txt e emoji-zwj-sequences.txt.

Saída: faixas ordenadas (u32, u32) para busca binária; nenhuma dependência.
"""
import os
import sys

VERSAO = '17.0.0'
MAX = 0x10FFFF

# Tabela de propriedades binárias do ECMAScript (nome canônico, apelidos).
# Só estes nomes valem em \p{...}; nada de casamento frouxo.
BINARIAS = [
    ('ASCII', []),
    ('ASCII_Hex_Digit', ['AHex']),
    ('Alphabetic', ['Alpha']),
    ('Any', []),
    ('Assigned', []),
    ('Bidi_Control', ['Bidi_C']),
    ('Bidi_Mirrored', ['Bidi_M']),
    ('Case_Ignorable', ['CI']),
    ('Cased', []),
    ('Changes_When_Casefolded', ['CWCF']),
    ('Changes_When_Casemapped', ['CWCM']),
    ('Changes_When_Lowercased', ['CWL']),
    ('Changes_When_NFKC_Casefolded', ['CWKCF']),
    ('Changes_When_Titlecased', ['CWT']),
    ('Changes_When_Uppercased', ['CWU']),
    ('Dash', []),
    ('Default_Ignorable_Code_Point', ['DI']),
    ('Deprecated', ['Dep']),
    ('Diacritic', ['Dia']),
    ('Emoji', []),
    ('Emoji_Component', ['EComp']),
    ('Emoji_Modifier', ['EMod']),
    ('Emoji_Modifier_Base', ['EBase']),
    ('Emoji_Presentation', ['EPres']),
    ('Extended_Pictographic', ['ExtPict']),
    ('Extender', ['Ext']),
    ('Grapheme_Base', ['Gr_Base']),
    ('Grapheme_Extend', ['Gr_Ext']),
    ('Hex_Digit', ['Hex']),
    ('IDS_Binary_Operator', ['IDSB']),
    ('IDS_Trinary_Operator', ['IDST']),
    ('ID_Continue', ['IDC']),
    ('ID_Start', ['IDS']),
    ('Ideographic', ['Ideo']),
    ('Join_Control', ['Join_C']),
    ('Logical_Order_Exception', ['LOE']),
    ('Lowercase', ['Lower']),
    ('Math', []),
    ('Noncharacter_Code_Point', ['NChar']),
    ('Pattern_Syntax', ['Pat_Syn']),
    ('Pattern_White_Space', ['Pat_WS']),
    ('Quotation_Mark', ['QMark']),
    ('Radical', []),
    ('Regional_Indicator', ['RI']),
    ('Sentence_Terminal', ['STerm']),
    ('Soft_Dotted', ['SD']),
    ('Terminal_Punctuation', ['Term']),
    ('Unified_Ideograph', ['UIdeo']),
    ('Uppercase', ['Upper']),
    ('Variation_Selector', ['VS']),
    ('White_Space', ['space']),
    ('XID_Continue', ['XIDC']),
    ('XID_Start', ['XIDS']),
]
# Montadas no regex.rs a partir de outras tabelas.
CALCULADAS = {'ASCII', 'Any', 'Assigned'}

# Propriedades de strings (flag v), na ordem do spec.
STRINGS = ['Basic_Emoji', 'Emoji_Keycap_Sequence', 'RGI_Emoji_Modifier_Sequence', 'RGI_Emoji_Flag_Sequence',
           'RGI_Emoji_Tag_Sequence', 'RGI_Emoji_ZWJ_Sequence']


def caminho(pasta, nome):
    return os.path.join(pasta, *nome.split('/'))


def conferir_versao(pasta, nome, marca):
    with open(caminho(pasta, nome), encoding='utf-8') as f:
        topo = ''.join(f.readline() for _ in range(10))
    if marca not in topo:
        sys.exit(f'{nome}: esperava a versão {marca}')


def ler(pasta, nome):
    """Linhas de dados (sem comentários), já separadas por ';'."""
    with open(caminho(pasta, nome), encoding='utf-8') as f:
        for linha in f:
            linha = linha.split('#', 1)[0].strip()
            if linha:
                yield [c.strip() for c in linha.split(';')]


def faixa(txt):
    if '..' in txt:
        a, b = txt.split('..')
        return int(a, 16), int(b, 16)
    v = int(txt, 16)
    return v, v


def juntar(faixas):
    out = []
    for a, b in sorted(faixas):
        if out and a <= out[-1][1] + 1:
            out[-1][1] = max(out[-1][1], b)
        else:
            out.append([a, b])
    return [tuple(x) for x in out]


def trechos(valores):
    """Lista por ponto de código → (inícios, valores) dos trechos iguais."""
    inicios, vals = [], []
    ant = None
    for cp, v in enumerate(valores):
        if v != ant:
            inicios.append(cp)
            vals.append(v)
            ant = v
    return inicios, vals


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    pasta, saida = sys.argv[1], sys.argv[2]
    for nome in ['PropertyAliases.txt', 'PropertyValueAliases.txt', 'Scripts.txt', 'ScriptExtensions.txt',
                 'PropList.txt', 'DerivedCoreProperties.txt', 'DerivedNormalizationProps.txt', 'CaseFolding.txt',
                 'SpecialCasing.txt', 'extracted/DerivedGeneralCategory.txt',
                 'extracted/DerivedBinaryProperties.txt']:
        conferir_versao(pasta, nome, VERSAO)
    for nome in ['emoji/emoji-data.txt', 'emoji-seq/emoji-sequences.txt', 'emoji-seq/emoji-zwj-sequences.txt']:
        conferir_versao(pasta, nome, 'Version: ' + VERSAO.rsplit('.', 1)[0])

    # ---------------------------------------------------- apelidos de valores
    gc_aliases = []  # (nomes, abreviação)
    sc_aliases = []  # (nomes, abreviação)
    for campos in ler(pasta, 'PropertyValueAliases.txt'):
        if campos[0] == 'gc':
            gc_aliases.append((campos[1:], campos[1]))
        elif campos[0] == 'sc':
            sc_aliases.append((campos[1:], campos[1]))
    prop_aliases = set()
    for campos in ler(pasta, 'PropertyAliases.txt'):
        prop_aliases.update(campos)
    for nome, apelidos in BINARIAS:
        for n in [nome] + apelidos:
            if n not in prop_aliases and n not in CALCULADAS:
                sys.exit(f'{n}: não está em PropertyAliases.txt')

    # ------------------------------------------------------ categoria geral
    folhas = [abrev for _, abrev in gc_aliases if len(abrev) == 2 and abrev != 'LC']
    idx_folha = {a: i for i, a in enumerate(folhas)}
    gc = [idx_folha['Cn']] * (MAX + 1)
    for campos in ler(pasta, 'extracted/DerivedGeneralCategory.txt'):
        a, b = faixa(campos[0])
        v = idx_folha[campos[1]]
        for cp in range(a, b + 1):
            gc[cp] = v
    grupos = {'C': 'C', 'L': 'L', 'M': 'M', 'N': 'N', 'P': 'P', 'S': 'S', 'Z': 'Z'}
    gc_mascara = {}
    for nomes, abrev in gc_aliases:
        if abrev == 'LC':
            m = sum(1 << idx_folha[x] for x in ('Lu', 'Ll', 'Lt'))
        elif abrev in grupos:
            m = sum(1 << i for f, i in idx_folha.items() if f[0] == abrev)
        else:
            m = 1 << idx_folha[abrev]
        for n in nomes:
            gc_mascara[n] = m
    gc_ini, gc_val = trechos(gc)

    # ------------------------------------------------------------- escritas
    sc_curto = {}  # nome longo ou apelido → abreviação
    escritas = []  # abreviações, na ordem dos índices
    for nomes, abrev in sc_aliases:
        escritas.append(abrev)
        for n in nomes:
            sc_curto[n] = abrev
    idx_esc = {a: i for i, a in enumerate(escritas)}
    sc = [idx_esc['Zzzz']] * (MAX + 1)
    for campos in ler(pasta, 'Scripts.txt'):
        a, b = faixa(campos[0])
        v = idx_esc[sc_curto[campos[1]]]
        for cp in range(a, b + 1):
            sc[cp] = v
    sc_ini, sc_val = trechos(sc)
    scx = {}
    for campos in ler(pasta, 'ScriptExtensions.txt'):
        a, b = faixa(campos[0])
        conj = tuple(sorted(idx_esc[sc_curto[x]] for x in campos[1].split()))
        for cp in range(a, b + 1):
            scx[cp] = conj
    conjuntos = sorted(set(scx.values()))
    idx_conj = {c: i for i, c in enumerate(conjuntos)}
    scx_faixas = []  # (início, fim, conjunto)
    for cp in sorted(scx):
        k = idx_conj[scx[cp]]
        if scx_faixas and scx_faixas[-1][1] == cp - 1 and scx_faixas[-1][2] == k:
            scx_faixas[-1][1] = cp
        else:
            scx_faixas.append([cp, cp, k])

    # ------------------------------------------------ propriedades binárias
    binarias = {nome: [] for nome, _ in BINARIAS}
    for arq in ['PropList.txt', 'DerivedCoreProperties.txt', 'DerivedNormalizationProps.txt',
                'extracted/DerivedBinaryProperties.txt', 'emoji/emoji-data.txt']:
        for campos in ler(pasta, arq):
            if len(campos) == 2 and campos[1] in binarias:
                binarias[campos[1]].append(faixa(campos[0]))
    for nome in binarias:
        binarias[nome] = juntar(binarias[nome])
        if not binarias[nome] and nome not in CALCULADAS:
            sys.exit(f'{nome}: sem dados')

    # -------------------------------------------------- propriedades de strings
    seqs = {n: [] for n in STRINGS}
    avulsos = []  # Basic_Emoji de um só ponto de código
    for arq in ['emoji-seq/emoji-sequences.txt', 'emoji-seq/emoji-zwj-sequences.txt']:
        for campos in ler(pasta, arq):
            tipo = campos[1]
            if tipo not in seqs:
                sys.exit(f'{arq}: tipo desconhecido {tipo}')
            cps = campos[0].split()
            if len(cps) == 1:
                if tipo != 'Basic_Emoji':
                    sys.exit(f'{arq}: {tipo} com um só caractere')
                avulsos.append(faixa(cps[0]))
            else:
                seqs[tipo].append(tuple(int(x, 16) for x in cps))
    avulsos = juntar(avulsos)

    # ------------------------------------------------------ dobra de caixa
    scf = {}
    for campos in ler(pasta, 'CaseFolding.txt'):
        if campos[1] in ('C', 'S'):
            scf[int(campos[0], 16)] = int(campos[2], 16)
    # Canonicalize sem u/v: toUppercase completo; vale só se der um caractere.
    especiais = {}
    for campos in ler(pasta, 'SpecialCasing.txt'):
        if len(campos) >= 5 and campos[4]:
            continue  # condicional (idioma/contexto): fora do toUppercase padrão
        especiais[int(campos[0], 16)] = [int(x, 16) for x in campos[3].split()]
    maiuscula = {}
    with open(caminho(pasta, 'UnicodeData.txt'), encoding='utf-8') as f:
        for linha in f:
            c = linha.split(';')
            if c[12]:
                maiuscula[int(c[0], 16)] = int(c[12], 16)
    canon = {}
    for cp in range(0x10000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        if cp in especiais:
            u = especiais[cp]
            if len(u) != 1:
                continue
            u = u[0]
        else:
            u = maiuscula.get(cp, cp)
        if u > 0xFFFF or (cp >= 128 and u < 128) or u == cp:
            continue
        canon[cp] = u
    # o regex.rs supõe canon(canon(c)) == canon(c)
    for tabela in (scf, canon):
        for k, v in tabela.items():
            if v in tabela:
                sys.exit(f'{k:X} -> {v:X} -> {tabela[v]:X}: canônico não é fixo')
    scf_inv = sorted((v, k) for k, v in scf.items())
    canon_inv = sorted((v, k) for k, v in canon.items())

    # ---------------------------------------------------------------- saída
    out = []
    w = out.append
    w('//! Tabelas Unicode do motor de regex (`regex.rs`).')
    w('//!')
    w(f'//! ARQUIVO GERADO por `js/tools/gen_unicode_props.py` a partir do Unicode')
    w(f'//! Character Database {VERSAO} (https://www.unicode.org/Public/{VERSAO}/ucd/ e')
    w(f'//! https://www.unicode.org/Public/{VERSAO}/emoji/). Não edite à mão: para')
    w('//! regenerar, baixe os arquivos listados no gerador numa pasta e rode')
    w('//! `python -I js/tools/gen_unicode_props.py <pasta> js/src/unicode_props.rs`.')
    w('')

    def lista_u32(nome, tipo, itens, doc):
        w(f'/// {doc}')
        w(f'pub static {nome}: [{tipo}; {len(itens)}] = [')
        linha = '   '
        for it in itens:
            txt = f' {it},'
            if len(linha) + len(txt) > 118:
                w(linha)
                linha = '   '
            linha += txt
        if linha.strip():
            w(linha)
        w('];')
        w('')

    def hx(v):
        return f'0x{v:X}'

    def pares(fx):
        return [f'({hx(a)}, {hx(b)})' for a, b in fx]

    lista_u32('GC_START', 'u32', [hx(x) for x in gc_ini],
              'Categoria geral: início de cada trecho (vai até o início do seguinte).')
    lista_u32('GC_VALUE', 'u8', gc_val, 'Categoria (índice da categoria folha) de cada trecho de `GC_START`.')
    w('// Categorias folha (valores de `GC_VALUE`, bits das máscaras de `GC_ALIASES`):')
    w('// ' + ', '.join(f'{f}={i}' for i, f in enumerate(folhas)))
    w('')
    lista_u32('GC_ALIASES', '(&str, u32)', [f'("{n}", 0x{m:X})' for n, m in sorted(gc_mascara.items())],
              'Valores de General_Category (nomes e apelidos) → máscara das categorias folha. Ordenada.')

    lista_u32('SC_START', 'u32', [hx(x) for x in sc_ini],
              'Escrita (Script): início de cada trecho (vai até o início do seguinte).')
    lista_u32('SC_VALUE', 'u8', sc_val, 'Escrita (índice em `SC_ALIASES`) de cada trecho de `SC_START`.')
    sc_nomes = sorted((n, idx_esc[a]) for n, a in sc_curto.items())
    lista_u32('SC_ALIASES', '(&str, u8)', [f'("{n}", {i})' for n, i in sc_nomes],
              'Valores de Script/Script_Extensions (nomes e apelidos) → índice da escrita. Ordenada.')
    lista_u32('SCX_RANGES', '(u32, u32, u16)', [f'({hx(a)}, {hx(b)}, {k})' for a, b, k in scx_faixas],
              'Script_Extensions onde difere de {Script}: (início, fim, índice em `SCX_SETS`).')
    w('/// Conjuntos de escritas usados por `SCX_RANGES`.')
    w(f'pub static SCX_SETS: [&[u8]; {len(conjuntos)}] = [')
    for c in conjuntos:
        w(f'    &[{", ".join(str(x) for x in c)}],')
    w('];')
    w('')

    nomes_rust = {}
    for nome, _ in BINARIAS:
        if nome in CALCULADAS:
            continue
        rn = 'BP_' + nome.upper()
        nomes_rust[nome] = rn
        lista_u32(rn, '(u32, u32)', pares(binarias[nome]), f'{nome}.')
    entradas = []
    for nome, apelidos in BINARIAS:
        for n in [nome] + apelidos:
            entradas.append((n, nome))
    entradas.sort()
    w('/// Propriedades binárias da tabela do ECMAScript (nome ou apelido → faixas).')
    w('/// `ASCII`, `Any` e `Assigned` ficam vazias aqui: o regex.rs as monta. Ordenada.')
    w(f'pub static BINARY: [(&str, &[(u32, u32)]); {len(entradas)}] = [')
    for n, nome in entradas:
        w(f'    ("{n}", &{nomes_rust[nome]}),' if nome in nomes_rust else f'    ("{n}", &[]),')
    w('];')
    w('')

    planas = []
    trechos_str = []
    for nome in STRINGS:
        ini = len(planas)
        for s in sorted(seqs[nome]):
            planas.append(str(len(s)))
            planas.extend(hx(x) for x in s)
        trechos_str.append((nome, ini, len(planas)))
    lista_u32('EMOJI_SEQS', 'u32', planas,
              'Sequências de emoji (propriedades de strings): o tamanho seguido dos pontos de código.')
    lista_u32('BASIC_EMOJI_SINGLES', '(u32, u32)', pares(avulsos), 'Basic_Emoji de um só ponto de código.')
    w('/// Propriedades de strings (flag v): nome → (trecho de `EMOJI_SEQS`, avulsos).')
    w(f'pub static STRING_PROPS: [(&str, (usize, usize), &[(u32, u32)]); {len(STRINGS) + 1}] = [')
    for nome, a, b in trechos_str:
        av = '&BASIC_EMOJI_SINGLES' if nome == 'Basic_Emoji' else '&[]'
        w(f'    ("{nome}", ({a}, {b}), {av}),')
    w(f'    ("RGI_Emoji", (0, {len(planas)}), &BASIC_EMOJI_SINGLES),')
    w('];')
    w('')

    lista_u32('SCF', '(u32, u32)', pares(sorted(scf.items())),
              'Dobra simples de caixa (CaseFolding.txt, status C e S): (código, dobrado). Ordenada.')
    lista_u32('SCF_INV', '(u32, u32)', pares(scf_inv), 'Inversa de `SCF`: (dobrado, código). Ordenada.')
    lista_u32('UPPER', '(u32, u32)', pares(sorted(canon.items())),
              'Canonicalize sem as flags u/v (toUppercase de um caractere só, sem cair no ASCII): '
              '(código, canônico). Ordenada.')
    lista_u32('UPPER_INV', '(u32, u32)', pares(canon_inv), 'Inversa de `UPPER`: (canônico, código). Ordenada.')

    with open(saida, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(out))
    tot_bin = sum(len(v) for v in binarias.values())
    print(f'gc: {len(gc_ini)} trechos, sc: {len(sc_ini)} trechos, scx: {len(scx_faixas)} faixas / '
          f'{len(conjuntos)} conjuntos, binárias: {tot_bin} faixas, strings: {len(planas)} u32 '
          f'({sum(len(v) for v in seqs.values())} sequências), avulsos: {len(avulsos)}, '
          f'scf: {len(scf)}, canon: {len(canon)}')


if __name__ == '__main__':
    main()

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

    def varint(v, out):
        while True:
            b = v & 0x7F
            v >>= 7
            if v:
                out.append(b | 0x80)
            else:
                out.append(b)
                return

    def faixas_bytes(fx):
        """Faixas → (distância desde o fim da anterior, tamanho - 1), em varint."""
        out = []
        prox = 0
        for a, b in fx:
            varint(a - prox, out)
            varint(b - a, out)
            prox = b + 1
        return out

    def trechos_bytes(inicios, vals):
        """Trechos contíguos → (tamanho - 1 em varint, valor)."""
        out = []
        for k, ini in enumerate(inicios):
            fim = inicios[k + 1] if k + 1 < len(inicios) else MAX + 1
            varint(fim - ini - 1, out)
            out.append(vals[k])
        return out

    def seqs_bytes(lista):
        """Sequências ordenadas, com prefixo comum: (comum, novos, pontos novos em varint)."""
        out = []
        ant = ()
        for sq in sorted(lista):
            comum = 0
            while comum < min(len(ant), len(sq)) and ant[comum] == sq[comum]:
                comum += 1
            out.append(comum)
            out.append(len(sq) - comum)
            for x in sq[comum:]:
                varint(x, out)
            ant = sq
        return out

    def bytes_(nome, dados, doc):
        lista_u32(nome, 'u8', [str(x) for x in dados], doc)

    def nomes(nome, lista, doc):
        w(f'/// {doc}')
        w(f'pub static {nome}: &str = "{" ".join(lista)}";')
        w('')

    w('// Formatos (o regex.rs decodifica ao compilar a regex):')
    w('// - faixas: pares (distância desde o fim da faixa anterior, tamanho - 1) em varint')
    w('// - trechos: (tamanho - 1 em varint, valor), cobrindo 0..=10FFFF em ordem')
    w('// - sequências: (prefixo comum com a anterior, quantos pontos novos, pontos em varint)')
    w('// varint = LEB128 sem sinal (7 bits por byte, bit alto = continua).')
    w('')
    bytes_('GC_RUNS', trechos_bytes(gc_ini, gc_val), 'Categoria geral em trechos (valor = categoria folha).')
    w('// Categorias folha (valores de `GC_RUNS`, bits das máscaras de `GC_MASKS`):')
    w('// ' + ', '.join(f'{f}={i}' for i, f in enumerate(folhas)))
    w('')
    gc_lista = sorted(gc_mascara.items())
    nomes('GC_NAMES', [n for n, _ in gc_lista], 'Valores de General_Category (nomes e apelidos), separados por espaço.')
    lista_u32('GC_MASKS', 'u32', [f'0x{m:X}' for _, m in gc_lista], 'Máscara das categorias folha de cada nome de `GC_NAMES`.')

    bytes_('SC_RUNS', trechos_bytes(sc_ini, sc_val), 'Escrita (Script) em trechos (valor = índice da escrita).')
    sc_nomes = sorted((n, idx_esc[a]) for n, a in sc_curto.items())
    nomes('SC_NAMES', [n for n, _ in sc_nomes], 'Valores de Script/Script_Extensions (nomes e apelidos), separados por espaço.')
    lista_u32('SC_INDEX', 'u8', [str(i) for _, i in sc_nomes], 'Índice da escrita de cada nome de `SC_NAMES`.')
    lista_u32('SCX_RANGES', '(u32, u32, u16)', [f'({hx(a)}, {hx(b)}, {k})' for a, b, k in scx_faixas],
              'Script_Extensions onde difere de {Script}: (início, fim, índice em `SCX_SETS`).')
    w('/// Conjuntos de escritas usados por `SCX_RANGES`.')
    w(f'pub static SCX_SETS: [&[u8]; {len(conjuntos)}] = [')
    for c in conjuntos:
        w(f'    &[{", ".join(str(x) for x in c)}],')
    w('];')
    w('')

    proprias = [nome for nome, _ in BINARIAS if nome not in CALCULADAS]
    for nome in proprias:
        bytes_('BP_' + nome.upper(), faixas_bytes(binarias[nome]), f'{nome} (faixas).')
    entradas = []
    for nome, apelidos in BINARIAS:
        for n in [nome] + apelidos:
            entradas.append((n, nome))
    entradas.sort()
    nomes('BINARY_NAMES', [n for n, _ in entradas],
          'Propriedades binárias da tabela do ECMAScript (nomes e apelidos), separadas por espaço.')
    idx = {nome: i for i, nome in enumerate(proprias)}
    lista_u32('BINARY_INDEX', 'u8', [str(idx.get(nome, 255)) for _, nome in entradas],
              'Propriedade (em `BINARY_DATA`) de cada nome de `BINARY_NAMES`; 255 = montada no regex.rs '
              '(`ASCII`, `Any`, `Assigned`).')
    w('/// Faixas de cada propriedade binária.')
    w(f'pub static BINARY_DATA: [&[u8]; {len(proprias)}] = [')
    for nome in proprias:
        w(f'    &BP_{nome.upper()},')
    w('];')
    w('')

    for nome in STRINGS:
        bytes_('SEQ_' + nome.upper(), seqs_bytes(seqs[nome]), f'{nome} (sequências).')
    bytes_('BASIC_EMOJI_SINGLES', faixas_bytes(avulsos), 'Basic_Emoji de um só ponto de código (faixas).')
    w('/// Propriedades de strings (flag v); `RGI_Emoji` é a união de todas.')
    w(f'pub static STRING_PROPS: [(&str, &[u8]); {len(STRINGS)}] = [')
    for nome in STRINGS:
        w(f'    ("{nome}", &SEQ_{nome.upper()}),')
    w('];')
    w('')

    lista_u32('SCF', '(u32, u32)', pares(sorted(scf.items())),
              'Dobra simples de caixa (CaseFolding.txt, status C e S): (código, dobrado). Ordenada.')
    lista_u32('UPPER', '(u32, u32)', pares(sorted(canon.items())),
              'Canonicalize sem as flags u/v (toUppercase de um caractere só, sem cair no ASCII): '
              '(código, canônico). Ordenada.')

    with open(saida, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(out))
    tot_bin = sum(len(v) for v in binarias.values())
    print(f'gc: {len(gc_ini)} trechos, sc: {len(sc_ini)} trechos, scx: {len(scx_faixas)} faixas / '
          f'{len(conjuntos)} conjuntos, binárias: {tot_bin} faixas, '
          f'{sum(len(v) for v in seqs.values())} sequências, avulsos: {len(avulsos)}, '
          f'scf: {len(scf)}, canon: {len(canon)}')


if __name__ == '__main__':
    main()

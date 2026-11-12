"""Teste diferencial: roda cada script de js/testes no Node e no Ferro JS e
compara a saída linha a linha.

Uso (da pasta ferro/):  python js/testes/diferencial.py [arquivos.js...]
Variáveis: MAXD=n (diferenças mostradas por arquivo), FERRO=caminho do binário.
"""
import glob
import os
import subprocess
import sys

aqui = os.path.dirname(os.path.abspath(__file__))
raiz = os.path.dirname(os.path.dirname(aqui))
ferro = os.environ.get('FERRO') or os.path.join(raiz, 'target', 'debug', 'ferro.exe' if os.name == 'nt' else 'ferro')
os.chdir(aqui)
arquivos = sys.argv[1:] or sorted(f for f in glob.glob('*.js') if f != 'bench.js')
maxd = int(os.environ.get('MAXD', '12'))


def saida(cmd):
    r = subprocess.run(cmd, capture_output=True, timeout=120)
    return (r.stdout + r.stderr).decode('utf-8', 'replace').replace('\r\n', '\n').splitlines()


total = 0
for f in arquivos:
    no = saida(['node', f])
    fe = saida([ferro, '--js', f])
    difs = []
    for i in range(max(len(no), len(fe))):
        a = no[i] if i < len(no) else '<nada>'
        b = fe[i] if i < len(fe) else '<nada>'
        if a != b:
            difs.append((i + 1, a, b))
    total += len(difs)
    print(f'== {f}: {len(no)} linhas no node, {len(fe)} no ferro, {len(difs)} diferenças')
    for (i, a, b) in difs[:maxd]:
        print(f'  L{i}\n    node : {a}\n    ferro: {b}')
print('TOTAL de linhas diferentes:', total)

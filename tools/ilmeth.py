import sys
from ildump import pe, R, resolve, CilMethodBody, Token
def dump_method(row):
    print("  -- method", row.Name)
    body=CilMethodBody(R(pe,row))
    for ins in body.instructions:
        arg=ins.operand
        a=resolve(arg) if isinstance(arg,Token) else (arg if arg is not None else '')
        print(f"     {ins.offset:5x} {ins.opcode.name} {a}")
for i,row in enumerate(pe.net.mdtables.MethodDef):
    if any(n == str(row.Name) or (n.endswith('*') and str(row.Name).startswith(n[:-1])) for n in sys.argv[1:]) and row.Rva:
        print('## rid', i+1); dump_method(row)

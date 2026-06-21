import csv, io

# reader
rows = list(csv.reader(['a,b,c', '1,2,3']))
print(rows[0])   # ['a', 'b', 'c']
print(rows[1])   # ['1', '2', '3']

# Quoted field with comma
rows = list(csv.reader(['"hello, world",b']))
print(rows[0])   # ['hello, world', 'b']

# Double-quote escape
rows = list(csv.reader(['"he said ""hi""",b']))
print(rows[0])   # ['he said "hi"', 'b']

# Empty fields
rows = list(csv.reader(['a,,c']))
print(rows[0])   # ['a', '', 'c']

# Empty quoted fields
print(list(csv.reader(['"",""'])))  # [['', '']]

# QUOTE_NONNUMERIC reader coerces unquoted to float
print(list(csv.reader(['1,2,3.5'], quoting=csv.QUOTE_NONNUMERIC)))  # [[1.0, 2.0, 3.5]]

# skipinitialspace
print(list(csv.reader(['a, b, c'], skipinitialspace=True)))  # [['a', 'b', 'c']]

# trailing blank line -> empty row
print(list(csv.reader(['a,b', ''])))  # [['a', 'b'], []]

# writer with \r\n
out = io.StringIO()
w = csv.writer(out)
w.writerow(['a', 'b', 'c'])
print(repr(out.getvalue()))  # 'a,b,c\r\n'

# writer quoting a comma-containing field
out2 = io.StringIO()
csv.writer(out2).writerow(['hello, world', 'b'])
print(repr(out2.getvalue()))  # '"hello, world",b\r\n'

# writer doubles embedded quotechar
out_q = io.StringIO()
csv.writer(out_q).writerow(['he"llo'])
print(repr(out_q.getvalue()))  # '"he""llo"\r\n'

# writer quotes embedded newline
out_n = io.StringIO()
csv.writer(out_n).writerow(['line1\nline2', 'b'])
print(repr(out_n.getvalue()))  # '"line1\nline2",b\r\n'

# writer coerces non-strings
out_i = io.StringIO()
csv.writer(out_i).writerow([1, 2, 3])
print(repr(out_i.getvalue()))  # '1,2,3\r\n'

# writer None -> empty field
out_none = io.StringIO()
csv.writer(out_none).writerow([None, 'a'])
print(repr(out_none.getvalue()))  # ',a\r\n'

# writerows
out_rows = io.StringIO()
csv.writer(out_rows).writerows([['a', 'b'], ['c', 'd']])
print(repr(out_rows.getvalue()))  # 'a,b\r\nc,d\r\n'

# QUOTE_ALL
out_all = io.StringIO()
csv.writer(out_all, quoting=csv.QUOTE_ALL).writerow(['a', 'b'])
print(repr(out_all.getvalue()))  # '"a","b"\r\n'

# QUOTE_NONNUMERIC writer
out_nn = io.StringIO()
csv.writer(out_nn, quoting=csv.QUOTE_NONNUMERIC).writerow(['a', 1, 2.5])
print(repr(out_nn.getvalue()))  # '"a",1,2.5\r\n'

# DictReader
rows = list(csv.DictReader(['name,age', 'Alice,30', 'Bob,25']))
print(rows[0]['name'], rows[0]['age'])  # Alice 30
print(rows[1]['name'], rows[1]['age'])  # Bob 25

# DictReader restval / restkey
rows = list(csv.DictReader(['a,b,c', '1,2', '4,5,6,7'], restval='X', restkey='extra'))
print(rows[0]['c'])      # X
print(rows[1]['extra'])  # ['7']

# DictReader fieldnames
dr = csv.DictReader(['n,a', 'x,1'])
print(dr.fieldnames)  # ['n', 'a']

# DictWriter
out3 = io.StringIO()
dw = csv.DictWriter(out3, fieldnames=['name', 'age'])
dw.writeheader()
dw.writerow({'name': 'Alice', 'age': 30})
print(repr(out3.getvalue()))  # 'name,age\r\nAlice,30\r\n'

# DictWriter missing key -> restval
out4 = io.StringIO()
dw4 = csv.DictWriter(out4, fieldnames=['a', 'b'])
dw4.writerow({'a': 1})
print(repr(out4.getvalue()))  # '1,\r\n'

# DictWriter extra key raises ValueError
out5 = io.StringIO()
dw5 = csv.DictWriter(out5, fieldnames=['a'])
try:
    dw5.writerow({'a': 1, 'z': 2})
except ValueError as e:
    print('VE', e)  # VE dict contains fields not in fieldnames: 'z'

# Constants
print(csv.QUOTE_MINIMAL)     # 0
print(csv.QUOTE_ALL)         # 1
print(csv.QUOTE_NONNUMERIC)  # 2
print(csv.QUOTE_NONE)        # 3

# list_dialects includes 'excel'
print('excel' in csv.list_dialects())  # True

# Tab delimiter via fmtparams
out6 = io.StringIO()
csv.writer(out6, delimiter='\t').writerow(['a', 'b'])
print(repr(out6.getvalue()))  # 'a\tb\r\n'

# reader dialect attribute
r = csv.reader(['a'])
print(r.dialect.delimiter)  # ,

print("csv ok")

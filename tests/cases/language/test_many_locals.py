# Function with many local variables — exercises the register allocator
# and verifies the MAX_SCRIPT_LOCALS fallback path (issue #53/#55).
def f():
    a0=0; a1=1; a2=2; a3=3; a4=4; a5=5; a6=6; a7=7; a8=8; a9=9
    b0=10; b1=11; b2=12; b3=13; b4=14; b5=15; b6=16; b7=17; b8=18; b9=19
    c0=20; c1=21; c2=22; c3=23; c4=24; c5=25; c6=26; c7=27; c8=28; c9=29
    d0=30; d1=31; d2=32; d3=33; d4=34; d5=35; d6=36; d7=37; d8=38; d9=39
    e0=40; e1=41; e2=42; e3=43; e4=44; e5=45; e6=46; e7=47; e8=48; e9=49
    return a0 + b9 + c5 + d3 + e7

print(f())  # 0 + 19 + 25 + 33 + 47 = 124

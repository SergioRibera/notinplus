import math
PAPER='#EFE6D4'; INK='#1F1B18'; RUBRIC='#C0482F'
def svg(body,vb="0 0 256 256",title="NotIn Plus"):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}"><title>{title}</title>{body}</svg>\n'
def rrect(x,y,w,h,r):
    return f'M{x+r},{y}H{x+w-r}A{r},{r} 0 0 1 {x+w},{y+r}V{y+h-r}A{r},{r} 0 0 1 {x+w-r},{y+h}H{x+r}A{r},{r} 0 0 1 {x},{y+h-r}V{y+r}A{r},{r} 0 0 1 {x+r},{y}Z'
# rule clipped to rounded tile (tile x0,y0,size s, radius r); rule from xa..xb
def rule_in_tile(xa,xb,x0=24,y0=24,s=208,r=52):
    cx=x0+r; yb0=y0+s
    def top(x): return y0+r-math.sqrt(r*r-(x-cx)**2) if x<cx else y0
    d=f"M{xa},{top(xa):.2f}"
    if xa<cx: d+=f" A{r},{r} 0 0 1 {min(xb,cx)},{top(min(xb,cx)):.2f}"
    if xb>cx: d+=f" H{xb}"
    d+=f" V{2*y0+s-top(xb):.2f}"
    if xb>cx: d+=f" H{max(xa,cx)}"
    if xa<cx: d+=f" A{r},{r} 0 0 1 {xa},{2*y0+s-top(xa):.2f}"
    return d+" Z"
# n glyph: monoline, optical centre slightly high
def n_path(dx=0,dy=0,s=1.0):
    P=lambda x,y: f'{(x*s+dx):.2f},{(y*s+dy):.2f}'
    return (f'M{P(0,0)} V{(80*s+dy):.2f} M{P(0,26)} C{P(0,8)} {P(14,-2)} {P(32,-2)} '
            f'C{P(50,-2)} {P(60,10)} {P(60,28)} V{(62*s+dy):.2f} C{P(60,74)} {P(66,80)} {P(76,80)} C{P(82,80)} {P(86,77)} {P(89,72)}')
def stroke(d,w,c): return f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>'
N=n_path(118,88)          # n: x 122..202(+tail), y 86..168
NS=stroke(N,18,INK)
# app icon (full-bleed square, platform masks it)
icon_full=f'<rect width="256" height="256" fill="{PAPER}"/><rect x="78" y="0" width="12" height="256" fill="{RUBRIC}"/>'+NS
# tile (rounded, for web/docs)
tile=f'<path fill="{PAPER}" d="{rrect(24,24,208,208,52)}"/><path fill="{RUBRIC}" d="{rule_in_tile(78,90)}"/>'+NS
# glyph only (no tile): rule + n
glyph=f'<rect x="78" y="40" width="12" height="176" rx="0" fill="{RUBRIC}"/>'+NS
# small-size cut (<=32px): heavier, bigger n, shorter tail
NSm=n_path(110,80,1.22).replace('C'+'','C')
small=(f'<path fill="{PAPER}" d="{rrect(24,24,208,208,52)}"/><path fill="{RUBRIC}" d="{rule_in_tile(64,80)}"/>'
       +stroke(n_path(114,80,1.16),26,INK))
open('icon-full.svg','w').write(svg(icon_full))
open('tile.svg','w').write(svg(tile))
open('glyph.svg','w').write(svg(glyph))
open('tile-small.svg','w').write(svg(small))

FONT='font-family="Noto Serif Display" font-weight="500" font-size="140"'
def word(x,y,ink=INK,rub=RUBRIC,anchor='start'):
    return f'<text x="{x}" y="{y}" {FONT} text-anchor="{anchor}" fill="{ink}">Not<tspan fill="{rub}">I</tspan>n Plus</text>'
open('src-word.svg','w').write(svg(word(0,177),"0 0 900 256"))
open('src-word-rev.svg','w').write(svg(word(0,177,PAPER,RUBRIC),"0 0 900 256"))
open('src-h.svg','w').write(svg(tile+word(280,177),"0 0 1200 256"))
open('src-h-rev.svg','w').write(svg(tile+word(280,177,PAPER,RUBRIC),"0 0 1200 256"))
open('src-v.svg','w').write(svg(f'<g transform="translate(222.5 0)">{tile}</g>'+word(-0.5,400),"0 0 700 460"))
open('src-v-rev.svg','w').write(svg(f'<g transform="translate(222.5 0)">{tile}</g>'+word(-0.5,400,PAPER,RUBRIC),"0 0 700 460"))
glyph_rev=f'<rect x="78" y="40" width="12" height="176" fill="{RUBRIC}"/>'+stroke(N,18,PAPER)
open('glyph-rev.svg','w').write(svg(glyph_rev))

# one-colour horizontal: glyph + wordmark, single ink
g1=f'<rect x="78" y="40" width="12" height="176" fill="{INK}"/>'+NS
open('src-h-mono.svg','w').write(svg(g1+word(250,177,INK,INK),"0 0 1200 256"))
# Android adaptive icon (108dp canvas, content scaled 0.8 around centre to sit in the 66dp safe zone)
k=108/256
bg=f'<rect width="108" height="108" fill="{PAPER}"/><rect x="{(128+(78-128)*0.8)*k:.3f}" y="0" width="{12*0.8*k:.3f}" height="108" fill="{RUBRIC}"/>'
fg=f'<g transform="scale({k:.5f}) translate(128 128) scale(0.8) translate(-128 -128)">{NS}</g>'
mono=f'<g transform="scale({k:.5f}) translate(128 128) scale(0.8) translate(-128 -128)"><rect x="78" y="40" width="12" height="176" fill="#000"/>{stroke(N,18,"#000")}</g>'
open('src-adaptive-bg.svg','w').write(svg(bg,"0 0 108 108"))
open('src-adaptive-fg.svg','w').write(svg(fg,"0 0 108 108"))
open('src-adaptive-mono.svg','w').write(svg(mono,"0 0 108 108"))

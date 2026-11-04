import re,sys
VB={'notinplus-horizontal-mono':'66 24 911 208','notinplus-wordmark':'4 66 693 116','notinplus-wordmark-reversed':'4 66 693 116',
    'notinplus-horizontal':'24 24 953 208','notinplus-horizontal-reversed':'24 24 953 208',
    'notinplus-stacked':'0 24 701 378','notinplus-stacked-reversed':'0 24 701 378'}
for f in sys.argv[1:]:
    s=open(f).read()
    name=f.rsplit('/',1)[-1][:-4]
    s=re.sub(r'<svg\b[^>]*>',lambda m:'<svg xmlns="http://www.w3.org/2000/svg" viewBox="%s">'%VB.get(name,'0 0 256 256'),s,count=1)
    s=re.sub(r'<(sodipodi|inkscape):[^>]*/>','',s)
    s=re.sub(r'<metadata.*?</metadata>','',s,flags=re.S)
    s=re.sub(r'<defs[^>]*/>','',s)
    s=re.sub(r'\s(id|inkscape:[\w-]+|sodipodi:[\w-]+|aria-label)="[^"]*"','',s)
    s=re.sub(r'\n\s*\n','\n',s)
    open(f,'w').write(s)

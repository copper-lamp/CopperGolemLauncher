
import os
from PIL import Image
im = Image.open(r"D:/CopperGolem/CopperCore/brand/app-win.png").convert("RGBA")
out = r"D:/CopperGolem/tmp-icon-test"
r = im.resize((256,256), Image.NEAREST)
r.save(f"{out}/256bmp.ico", format="ICO", sizes=[(256,256)], bitmap_format="bmp")
d=open(f"{out}/256bmp.ico","rb").read()
print(len(d), d[22:26], d[22:30].hex())
d2=open(f"{out}/256.ico","rb").read()
print(len(d2), d2[22:26], d2[22:30].hex())
import PIL; print(PIL.__version__)

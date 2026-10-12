# Developer-only asset conversion. The application embeds app.ico and does not
# need PowerShell, System.Drawing or the source PNG at runtime.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Add-Type -AssemblyName System.Drawing
$references = if (Test-Path "$PSHOME/ref") {
    @(Get-ChildItem "$PSHOME/ref/*.dll" | ForEach-Object FullName) + @([Drawing.Bitmap].Assembly.Location)
} else { @('System.Drawing') }
foreach ($assembly in @('System.Private.Windows.GdiPlus.dll','System.Private.Windows.Core.dll')) {
    if (Test-Path "$PSHOME/$assembly") { $references += "$PSHOME/$assembly" }
}
Add-Type -ReferencedAssemblies $references @'
using System;
using System.IO;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Collections.Generic;
public static class GatewayIconAsset {
    static Bitmap Resize(Image source, int size) {
        var bitmap=new Bitmap(size,size,PixelFormat.Format32bppArgb);
        using(var graphics=Graphics.FromImage(bitmap)) {
            graphics.CompositingMode=CompositingMode.SourceCopy;
            graphics.InterpolationMode=InterpolationMode.HighQualityBicubic;
            graphics.PixelOffsetMode=PixelOffsetMode.HighQuality;
            using(var attributes=new ImageAttributes()) {
                attributes.SetWrapMode(WrapMode.TileFlipXY);
                graphics.DrawImage(source,new Rectangle(0,0,size,size),0,0,source.Width,source.Height,GraphicsUnit.Pixel,attributes);
            }
        }
        return bitmap;
    }
    public static void Build(string directory) {
        int[] sizes={16,20,24,32,40,48,64,128,256};
        using(var original=new Bitmap(Path.Combine(directory,"icon-original.png"))) {
            int left=original.Width,top=original.Height,right=0,bottom=0;
            for(int y=0;y<original.Height;y++) for(int x=0;x<original.Width;x++) {
                var color=original.GetPixel(x,y);
                if(color.A<12) original.SetPixel(x,y,Color.Transparent);
                if(color.A>=64) {left=Math.Min(left,x);top=Math.Min(top,y);right=Math.Max(right,x);bottom=Math.Max(bottom,y);}
            }
            if(right<=left||bottom<=top) throw new InvalidDataException("Generated image has no opaque symbol");
            var bounds=new Rectangle(Math.Max(0,left-2),Math.Max(0,top-2),Math.Min(original.Width-left+2,right-left+5),Math.Min(original.Height-top+2,bottom-top+5));
            using(var cropped=original.Clone(bounds,PixelFormat.Format32bppArgb))
            using(var master=new Bitmap(1024,1024,PixelFormat.Format32bppArgb)) {
                float ratio=778f/Math.Max(cropped.Width,cropped.Height);
                int width=(int)Math.Round(cropped.Width*ratio),height=(int)Math.Round(cropped.Height*ratio);
                using(var graphics=Graphics.FromImage(master)) {
                    graphics.InterpolationMode=InterpolationMode.HighQualityBicubic;
                    graphics.PixelOffsetMode=PixelOffsetMode.HighQuality;
                    graphics.DrawImage(cropped,new Rectangle((1024-width)/2,(1024-height)/2,width,height));
                }
                master.Save(Path.Combine(directory,"icon-source.png"),ImageFormat.Png);
                var frames=new List<byte[]>();
                foreach(int size in sizes) using(var image=Resize(master,size)) using(var stream=new MemoryStream()) {image.Save(stream,ImageFormat.Png);frames.Add(stream.ToArray());}
                using(var writer=new BinaryWriter(File.Create(Path.Combine(directory,"app.ico")))) {
                    writer.Write((ushort)0);writer.Write((ushort)1);writer.Write((ushort)sizes.Length);
                    int offset=6+16*sizes.Length;
                    for(int i=0;i<sizes.Length;i++) {
                        writer.Write((byte)(sizes[i]==256?0:sizes[i]));writer.Write((byte)(sizes[i]==256?0:sizes[i]));
                        writer.Write((byte)0);writer.Write((byte)0);writer.Write((ushort)1);writer.Write((ushort)32);
                        writer.Write(frames[i].Length);writer.Write(offset);offset+=frames[i].Length;
                    }
                    foreach(var frame in frames) writer.Write(frame);
                }
                using(var preview=new Bitmap(900,440,PixelFormat.Format32bppArgb))
                using(var graphics=Graphics.FromImage(preview))
                using(var font=new Font("Segoe UI",11)) {
                    for(int row=0;row<2;row++) {
                        graphics.FillRectangle(row==0?Brushes.White:new SolidBrush(Color.FromArgb(28,32,40)),0,row*220,900,220);
                        for(int i=0;i<sizes.Length;i++) {
                            int x=30+i*96,y=row*220+54;
                            using(var image=Resize(master,sizes[i])) {
                                if(sizes[i]<=64) graphics.DrawImageUnscaled(image,x+(64-sizes[i])/2,y+(64-sizes[i])/2);
                                else graphics.DrawImage(image,x,y,64,64);
                            }
                            graphics.DrawString(sizes[i]+" px",font,row==0?Brushes.Black:Brushes.White,x,row*220+132);
                        }
                        graphics.DrawString(row==0?"LIGHT / native sizes 16-64":"DARK / 128-256 scaled for preview",font,row==0?Brushes.Black:Brushes.White,24,row*220+16);
                    }
                    preview.Save(Path.Combine(directory,"icon-preview.png"),ImageFormat.Png);
                }
            }
        }
    }
}
'@
[GatewayIconAsset]::Build("$root/assets")
Write-Host 'Generated 1024px PNG, nine-size ICO and light/dark preview'

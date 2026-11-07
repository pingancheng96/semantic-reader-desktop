import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let size = 1024
let rgb = CGColorSpace(name: CGColorSpace.sRGB)!
let canvas = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                       bytesPerRow: 0, space: rgb, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(red: CGFloat((hex >> 16) & 255)/255, green: CGFloat((hex >> 8) & 255)/255,
            blue: CGFloat(hex & 255)/255, alpha: alpha)
}
func fill(_ path: CGPath, _ hex: UInt32, _ alpha: CGFloat = 1) {
    canvas.addPath(path); canvas.setFillColor(color(hex,alpha)); canvas.fillPath()
}
func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x:x,y:y) }
func round(_ x: CGFloat,_ y: CGFloat,_ w: CGFloat,_ h: CGFloat,_ radius: CGFloat) -> CGPath {
    CGPath(roundedRect:CGRect(x:x,y:y,width:w,height:h),cornerWidth:radius,cornerHeight:radius,transform:nil)
}
func line(_ points:[CGPoint], _ width:CGFloat, _ hex:UInt32, _ alpha:CGFloat = 1) {
    canvas.setLineCap(.round); canvas.setLineJoin(.round)
    canvas.setStrokeColor(color(hex,alpha)); canvas.setLineWidth(width)
    canvas.addLines(between:points); canvas.strokePath()
}
func dot(_ x: CGFloat,_ y: CGFloat,_ r: CGFloat,_ hex:UInt32) {
    canvas.setFillColor(color(hex));canvas.fillEllipse(in:CGRect(x:x-r,y:y-r,width:r*2,height:r*2))
}

let tile = round(64,64,896,896,206)
canvas.saveGState()
canvas.setShadow(offset:CGSize(width:0,height:-12),blur:22,color:color(0x102820,0.34))
fill(tile,0x163a33)
canvas.restoreGState()
canvas.saveGState()
canvas.addPath(tile);canvas.clip()
let background = CGGradient(colorsSpace:rgb,colors:[color(0x326b59),color(0x163a33),color(0x102b27)] as CFArray,
                            locations:[0,0.63,1])!
canvas.drawLinearGradient(background,start:point(160,920),end:point(855,97),options:[])
canvas.setStrokeColor(color(0x9ed6b5,0.22));canvas.setLineWidth(3)
canvas.addPath(round(69,69,886,886,201));canvas.strokePath()
canvas.restoreGState()

let left = CGMutablePath()
left.move(to:point(512,277))
left.addQuadCurve(to:point(245,300),control:point(387,329))
left.addQuadCurve(to:point(215,330),control:point(215,293))
left.addLine(to:point(215,674))
left.addQuadCurve(to:point(245,702),control:point(215,696))
left.addQuadCurve(to:point(512,661),control:point(384,736))
left.closeSubpath()
let right = CGMutablePath()
right.move(to:point(512,277))
right.addQuadCurve(to:point(779,300),control:point(638,329))
right.addQuadCurve(to:point(809,330),control:point(809,293))
right.addLine(to:point(809,674))
right.addQuadCurve(to:point(779,702),control:point(809,696))
right.addQuadCurve(to:point(512,661),control:point(640,736))
right.closeSubpath()
let book = CGMutablePath();book.addPath(left);book.addPath(right)
canvas.saveGState()
canvas.setShadow(offset:CGSize(width:0,height:-16),blur:28,color:color(0x0a201b,0.56))
fill(book,0xe3efd9)
canvas.restoreGState()
fill(left,0xf1f3df);fill(right,0xd6e8cd)
line([point(512,294),point(512,651)],7,0x74927a,0.74)
let textLines: [(CGFloat, CGFloat)] = [(615,139),(550,119),(423,139),(368,91)]
for (y,width) in textLines {
    line([point(267,y),point(267+width,y+7)],13,0x4d7663,0.53)
}
fill(round(257,462,175,46,19),0xf3be65)
line([point(275,484),point(405,484)],11,0x745127,0.77)
line([point(600,420),point(705,531),point(661,621)],15,0x326851)
line([point(705,531),point(750,436)],15,0x326851)
dot(600,420,22,0x37775d)
dot(661,621,24,0x37775d)
dot(750,436,22,0x37775d)
dot(705,531,32,0xf3be65)
dot(705,531,11,0x895a2c)

let output = URL(fileURLWithPath: CommandLine.arguments[1])
let destination = CGImageDestinationCreateWithURL(output as CFURL,UTType.png.identifier as CFString,1,nil)!
CGImageDestinationAddImage(destination,canvas.makeImage()!,nil)
guard CGImageDestinationFinalize(destination) else { fatalError("Could not create PNG") }

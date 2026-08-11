import Foundation
import UIKit

let arguments = CommandLine.arguments
guard arguments.count == 4 else {
  fatalError("usage: GenerateFixtures <source.png> <output.png> <output.jpg>")
}

let source = try Data(contentsOf: URL(fileURLWithPath: arguments[1]))
guard
  let image = UIImage(data: source),
  let png = image.pngData(),
  let jpeg = image.jpegData(compressionQuality: 1.0)
else {
  fatalError("UIKit could not encode the synthetic source image")
}

try png.write(to: URL(fileURLWithPath: arguments[2]))
try jpeg.write(to: URL(fileURLWithPath: arguments[3]))

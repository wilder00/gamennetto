# Third-party software

## Virtual Display Driver (extend mode)
`gamennetto display install` downloads and installs this driver. It is not
part of Gamennetto's source code and runs as a separate driver.

- Project: https://github.com/VirtualDrivers/Virtual-Display-Driver
- Version: 25.7.23 (file `VirtualDisplayDriver-x86.Driver.Only.zip`,
  SHA-256 `e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a`)
- Signed by SignPath Foundation
- Licence: MIT, "Copyright (c) 2024 Virtual Display"

  > Permission is hereby granted, free of charge, to any person obtaining a copy
  > of this software and associated documentation files (the "Software"), to deal
  > in the Software without restriction, including without limitation the rights
  > to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  > copies of the Software, and to permit persons to whom the Software is
  > furnished to do so, subject to the following conditions:
  >
  > The above copyright notice and this permission notice shall be included in all
  > copies or substantial portions of the Software.
  >
  > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  > IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  > FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  > AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  > LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  > SOFTWARE.

- The driver is based on Microsoft's IndirectDisplay sample from
  Windows-driver-samples, licensed under the Microsoft Public License (MS-PL):
  https://github.com/microsoft/Windows-driver-samples/blob/main/LICENSE

TODO before shipping an installer that bundles the driver: include the full
MS-PL text alongside it.

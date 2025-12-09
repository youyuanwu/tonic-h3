# Buck2
Support building with buck2.
```ps1
Invoke-WebRequest -Uri https://github.com/facebook/buck2/releases/download/2025-12-01/buck2-x86_64-pc-windows-msvc.exe.zst -OutFile "buck2-x86_64-pc-windows-msvc.exe.zst"

# extract it via ui.
Copy-Item .\buck2-x86_64-pc-windows-msvc.exe\buck2-x86_64-pc-windows-msvc.exe ${HOME}\.cargo\bin\buck2.exe

Remove-Item buck2-x86_64-pc-windows-msvc.exe.zst 
Remove-Item buck2-x86_64-pc-windows-msvc.exe

Invoke-WebRequest -Uri https://github.com/facebookincubator/reindeer/releases/download/v2025.11.24.00/reindeer-x86_64-pc-windows-msvc.exe.zst -OutFile "reindeer-x86_64-pc-windows-msvc.exe.zst"
# extract it via ui.
Copy-Item .\reindeer-x86_64-pc-windows-msvc.exe\reindeer-x86_64-pc-windows-msvc.exe ${HOME}\.cargo\bin\reindeer.exe

Remove-Item reindeer-x86_64-pc-windows-msvc.exe.zst
Remove-Item reindeer-x86_64-pc-windows-msvc.exe
```

Generate:
```
reindeer buckify
```
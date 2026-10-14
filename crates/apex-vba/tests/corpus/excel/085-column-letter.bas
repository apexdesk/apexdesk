' corpus-source: generated
' corpus-flagged: no
' corpus-prints: C
' corpus-prints: AB
Function ColLetter(n As Long) As String
    Dim s As String
    Do While n > 0
        s = Chr(65 + (n - 1) Mod 26) & s
        n = (n - 1) \ 26
    Loop
    ColLetter = s
End Function
Sub Main()
    Debug.Print ColLetter(3)
    Debug.Print ColLetter(28)
End Sub
